# Internal adversarial pass — argued AGAINST `remap-actionable`

I walked in believing `remap-actionable`, because the capture is uniformly clean.
This pass argues against it.

## The case for `remap-opaque`

All four backends produce `REMAP-DISPOSITION: refused` and `REMAP-EXIT: 1`.
That uniformity is itself suspicious. `plaintext` keeps its secrets IN the tree;
`keyring` keeps them in the OS keychain where the archive cannot reach them at
all. Those are materially different situations for an operator, and a contract
that answers both with an identical refusal is arguably telling the operator
less than it appears to. The `REMAP-NAMES-*` fields are booleans — they record
THAT the backend, the count and an action were named, not that the named action
is correct or achievable. A message could name an action that does not work and
still score `yes` on all three.

## The case for `remap-warns-and-continues`

Not available on the evidence, and worth stating why so it is not left implied.
`REMAP-TARGET-WRITTEN: no` appears for all four, and the capture obtains it by
digesting the target before and after rather than by reading the message — which
is exactly how a warn-and-continue would be caught. Four refusals, zero writes.

## Why I do not adopt `remap-opaque`

The uniform refusal is not uniform ignorance; it is the default posture, and it
is correct. Every one of these restores is a CROSS-MACHINE restore of an archive
that, by default, is redacted — so in every case some credential source will be
absent on the target. Refusing by default in all four is the contract working,
and `--accept-missing-secrets` is the documented way through it. The verbatim
messages differ per backend where the situation differs: the `keyring` message
names "keyring-stored secrets (OS keychain)" and directs the operator to
`wayland-core auth add <provider>`, which is a real, runnable command.

The stronger half of the opacity argument survives and I am recording it rather
than dismissing it: the `REMAP-NAMES-*` fields measure presence, not
correctness. Nothing in this capture proves the prescribed action actually
restores a working install — only that an action was named. That is a genuine
limit of this evidence and it belongs in the decision record.

## Conclusion

`remap-actionable`, with the recorded limit that the naming checks are presence
checks and the prescribed recovery action is not itself executed by any gate.
