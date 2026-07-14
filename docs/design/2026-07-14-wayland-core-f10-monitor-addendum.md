# F10 Mid-Flight Monitor Scope Addendum

F10 governs observable no-progress after provider attempts and tool outcomes.
It owns normalized repeated errors, normalized one-step and multi-step tool
routes, repeated completed provider attempts with no output after a failed tool
round, and run-budget decisions. A replan is claimed only after its directive is
committed to the next provider request; an ignored repeated route stops with a
Continue-able `max_turns` finish.

F10 does not claim an absolute timeout around a provider future that never opens
a stream or a stream receiver that never yields an event. F15 explicitly owns
provider request/stream stalls, timeout policy, retry/failover semantics, and
cooldown integration. Host cancellation already interrupts both waits; the
remaining no-host-cancel hang is an explicit F15 boundary, not an F10 pass.

Success-outcome normalization preserves semantic numbers such as improving test
counts. It removes only explicit volatile metadata (for example request IDs,
nonces, PIDs, UUIDs, and RFC3339 timestamps). Error normalization is broader but
preserves status codes, URL authorities, path scope, resource identity, semantic
dates, and version-like values. Exact single-call repetition remains owned by
LoopGuard; the monitor owns one-step repetition only when raw outcomes vary but
collapse after volatile-field normalization.

The host-visible `mid_flight_monitor_decision` protocol event carries typed
`replan` or `stop` directives and stable reasons. It is always-on additive wire
surface governed by the JSON-stream Host Decoder Contract.
