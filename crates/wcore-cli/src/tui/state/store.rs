//! A minimal subscriber store (CC `src/state/store.ts` reimplemented).
//!
//! SPEC §1B. `set` short-circuits when the next value equals the previous
//! (`PartialEq ==`, the `Object.is` equivalent), firing no listeners — the
//! no-op-update guard. Otherwise it fires `on_change(&next, &prev)` THEN the
//! listeners, THEN commits `self.state = next`. `select` reads a derived
//! slice for the render loop's redraw-skip decision (`dirty_if_changed`).
//!
//! v0.9.2 W10 wraps ONLY the transient cost/mcp/context/toast slice in this
//! store (the §10 risk-2 conservative migration); the bulk `App` /
//! `SessionView` migration is a v0.9.4 follow-up.

/// A minimal subscriber store. Generic over a `PartialEq + Clone` state so
/// the no-op guard can compare `next == prev` before firing anything.
///
/// Listeners are `Fn(&T)` (the new state) and `on_change` is `Fn(&T, &T)`
/// (`next`, `prev`) — matching CC's `setState` ordering: `onChange` runs
/// before the subscriber fan-out, both before the commit.
// v0.9.2 W10: the listener / on_change trait objects are `+ Send` so a
// `Store` (and the `App` that owns one) stays `Send` for the
// `Arc<Mutex<App>>` shared with the tokio bridge task. Single-threaded
// callers (every current subscriber) satisfy `Send` trivially.
//
// `type_complexity` is allowed: the boxed-`Fn` storage is the SPEC §1B
// design verbatim (CC's `setState`/`subscribe` store); a `type` alias would
// just relocate the same complexity behind a name that hurts readability.
#[allow(clippy::type_complexity)]
pub struct Store<T> {
    state: T,
    listeners: Vec<Box<dyn Fn(&T) + Send>>,
    on_change: Option<Box<dyn Fn(&T, &T) + Send>>,
}

impl<T: PartialEq + Clone> Store<T> {
    /// Construct the store with an initial value and an optional
    /// `on_change(next, prev)` hook fired (once) ahead of the listeners on
    /// every value-changing `set`.
    // SPEC §1B signature verbatim; the boxed-`Fn` is intrinsic to the design.
    #[allow(clippy::type_complexity)]
    pub fn new(initial: T, on_change: Option<Box<dyn Fn(&T, &T) + Send>>) -> Self {
        Self {
            state: initial,
            listeners: Vec::new(),
            on_change,
        }
    }

    /// Read the current state by reference. Free — no clone.
    pub fn get(&self) -> &T {
        &self.state
    }

    /// Apply `updater` to compute `next = updater(&prev)`.
    ///
    /// The `Object.is` no-op guard: if `next == prev`, return WITHOUT firing
    /// `on_change` or any listener and WITHOUT committing — a no-op write is
    /// invisible. Otherwise fire `on_change(&next, &prev)`, then every
    /// listener with `&next`, then commit `self.state = next`.
    pub fn set(&mut self, updater: impl FnOnce(&T) -> T) {
        let next = updater(&self.state);
        if next == self.state {
            return; // no-op guard — nothing changed, fire nothing.
        }
        if let Some(cb) = &self.on_change {
            cb(&next, &self.state);
        }
        for l in &self.listeners {
            l(&next);
        }
        self.state = next;
    }

    /// Silently overwrite the stored state WITHOUT firing `on_change` or any
    /// listener. This is the escape hatch for the conservative W10 migration:
    /// when canonical fields are written DIRECTLY (bypassing `set`), the
    /// store's internal `state` drifts from canonical, and the `set` no-op
    /// guard then compares against a stale value (audit M3). Calling
    /// `reseed(canonical)` before the real `set` realigns the internal state
    /// with canonical so the very next `set` compares `next == canonical` —
    /// a real change always bumps the revision, an identical one never does.
    /// No subscriber fires here, so reseeding alone never bumps the revision.
    pub fn reseed(&mut self, state: T) {
        self.state = state;
    }

    /// Register a listener fired (with the new state) on every
    /// value-changing `set`. Listeners are never fired by a no-op `set`.
    pub fn subscribe(&mut self, listener: impl Fn(&T) + Send + 'static) {
        self.listeners.push(Box::new(listener));
    }

    /// Read a derived slice of the state. The render loop uses this to take
    /// a cheap, comparable snapshot of the part of the state it cares about
    /// (the redraw-skip selector) without cloning the whole state.
    pub fn select<S: PartialEq>(&self, sel: impl Fn(&T) -> S) -> S {
        sel(&self.state)
    }

    /// `dirty_if_changed`: compare a previously-`select`ed slice value
    /// against the current one. Returns `true` when the selected slice has
    /// changed since `prev` was taken — i.e. the render loop must redraw.
    /// Pure (no mutation), so the run loop can call it every iteration to
    /// decide whether the transcript-independent transient row needs a
    /// fresh paint.
    pub fn dirty_if_changed<S: PartialEq>(&self, prev: &S, sel: impl Fn(&T) -> S) -> bool {
        &sel(&self.state) != prev
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    // Listeners are `+ Send` (so a `Store` is `Send` for the
    // `Arc<Mutex<App>>` shared with the bridge task). The test side-channels
    // therefore use `Arc<Atomic*>` / `Arc<Mutex<_>>`, not `Rc<Cell<_>>`.

    #[test]
    fn no_op_set_does_not_fire_listeners() {
        let fired = Arc::new(AtomicU64::new(0));
        let mut s = Store::new(5u32, None);
        let f = fired.clone();
        s.subscribe(move |_| {
            f.fetch_add(1, Ordering::Relaxed);
        });
        s.set(|_| 5); // no-op (== prev) → no fire
        assert_eq!(fired.load(Ordering::Relaxed), 0);
        s.set(|v| v + 1); // changed → fire once
        assert_eq!(fired.load(Ordering::Relaxed), 1);
        assert_eq!(*s.get(), 6);
    }

    #[test]
    fn no_op_set_does_not_commit_or_fire_on_change() {
        // on_change must also be skipped by the no-op guard, and the state
        // must be left exactly as it was.
        let on_change_fired = Arc::new(AtomicU64::new(0));
        let oc = on_change_fired.clone();
        let mut s = Store::new(
            42u32,
            Some(Box::new(move |_next: &u32, _prev: &u32| {
                oc.fetch_add(1, Ordering::Relaxed);
            })),
        );
        s.set(|v| *v); // identical value → no-op
        assert_eq!(
            on_change_fired.load(Ordering::Relaxed),
            0,
            "no-op must not fire on_change"
        );
        assert_eq!(*s.get(), 42, "no-op must not change the committed state");
    }

    #[test]
    fn on_change_fires_before_listeners_and_sees_next_and_prev() {
        // Ordering contract: on_change(next, prev) runs first, then the
        // listeners, then commit. Record an interleaving log to prove it.
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let oc_log = log.clone();
        let mut s = Store::new(
            1u32,
            Some(Box::new(move |next: &u32, prev: &u32| {
                oc_log
                    .lock()
                    .unwrap()
                    .push(format!("on_change:{prev}->{next}"));
            })),
        );
        let l_log = log.clone();
        s.subscribe(move |next| l_log.lock().unwrap().push(format!("listener:{next}")));
        s.set(|v| v + 9); // 1 -> 10
        let recorded = log.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec!["on_change:1->10".to_string(), "listener:10".to_string()],
            "on_change must run before listeners, both seeing next=10 prev=1"
        );
        assert_eq!(*s.get(), 10, "commit happens after the fan-out");
    }

    #[test]
    fn select_reads_a_derived_slice() {
        let s = Store::new((3u32, "hi".to_string()), None);
        assert_eq!(s.select(|v| v.0), 3);
        assert_eq!(s.select(|v| v.1.len()), 2);
    }

    #[test]
    fn dirty_if_changed_detects_a_changed_slice() {
        let mut s = Store::new((0u32, 0u32), None);
        let prev = s.select(|v| v.0); // snapshot the first field
        // Mutate only the SECOND field — the selected slice is unchanged.
        s.set(|v| (v.0, v.1 + 1));
        assert!(
            !s.dirty_if_changed(&prev, |v| v.0),
            "first field unchanged → not dirty for that selector"
        );
        // Now mutate the first field — the selector is dirty.
        s.set(|v| (v.0 + 1, v.1));
        assert!(
            s.dirty_if_changed(&prev, |v| v.0),
            "first field changed → dirty for that selector"
        );
    }

    #[test]
    fn reseed_is_silent_and_realigns_the_no_op_guard() {
        // v0.9.2 audit M3: `reseed` overwrites the stored state WITHOUT firing
        // anything — and it realigns the no-op guard so the next `set`
        // compares against the reseeded (canonical) value, not a stale one.
        let fired = Arc::new(AtomicU64::new(0));
        let mut s = Store::new(0u32, None);
        let f = fired.clone();
        s.subscribe(move |_| {
            f.fetch_add(1, Ordering::Relaxed);
        });

        // Simulate a direct write that bypassed `set`: the canonical value is
        // 7, but the store's internal state is still 0 (drifted).
        // `reseed(7)` realigns silently — no listener fires.
        s.reseed(7);
        assert_eq!(fired.load(Ordering::Relaxed), 0, "reseed fires nothing");
        assert_eq!(*s.get(), 7, "reseed overwrites the stored state");

        // A `set` to the SAME canonical value (7) is now correctly a no-op —
        // before the reseed it would have compared 7 != stale-0 and fired
        // spuriously. After the reseed: no fire.
        s.set(|_| 7);
        assert_eq!(
            fired.load(Ordering::Relaxed),
            0,
            "set to the reseeded value is a true no-op"
        );

        // A `set` to a genuinely different value fires exactly once — the M3
        // failure (a real change short-circuited by stale state) cannot happen
        // because the guard now compares against canonical.
        s.set(|_| 9);
        assert_eq!(
            fired.load(Ordering::Relaxed),
            1,
            "a real change after reseed bumps exactly once"
        );
        assert_eq!(*s.get(), 9);
    }

    #[test]
    fn multiple_listeners_all_fire_on_change() {
        let a = Arc::new(AtomicU64::new(0));
        let b = Arc::new(AtomicU64::new(0));
        let mut s = Store::new(0u32, None);
        let (ca, cb) = (a.clone(), b.clone());
        s.subscribe(move |_| {
            ca.fetch_add(1, Ordering::Relaxed);
        });
        s.subscribe(move |_| {
            cb.fetch_add(1, Ordering::Relaxed);
        });
        s.set(|v| v + 1);
        assert_eq!(
            (a.load(Ordering::Relaxed), b.load(Ordering::Relaxed)),
            (1, 1)
        );
        s.set(|v| *v); // no-op — neither fires again
        assert_eq!(
            (a.load(Ordering::Relaxed), b.load(Ordering::Relaxed)),
            (1, 1)
        );
    }
}
