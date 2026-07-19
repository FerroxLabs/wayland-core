# Core to Desktop Protocol Checkpoints

Desktop is the primary GUI/control plane; Core remains the enforcement authority and independently usable CLI/TUI engine. Core cannot claim Desktop behavior from Core tests alone.

## D1 - required after Phase 20 and before Phase 21 execution

Before Phase 21 broad execution, publish a linked Desktop plan and host conformance suite consuming a pinned clean Core producer. Record protocol version, fixture/schema digests, generator version, EffectiveExecutionPolicy semantics, lifecycle/correlation/ordering/duplicate/terminal/failure behavior, and ownership boundaries. Issue coordination is not completion.

## D2 - Phase 23 exit gate

Freeze durable Goal/child/task/wait commands, events, cursors, delivery, approval, failure and reconnect semantics. Replay canonical serialized fixtures through the real Desktop consumer/reducer. Deserialization alone is insufficient. Desktop may then implement the background-work control plane without inventing a second lifecycle.

Core Phase 22 proves producer fixtures and standalone/host protocol behavior. The linked Desktop lane proves consumer replay and UI/control behavior. Both receipts are required for a whole-Wayland claim; neither blocks Core-only engine claims outside the shared contract.
