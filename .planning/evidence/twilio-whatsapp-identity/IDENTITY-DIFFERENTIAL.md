# The measurement: does a Twilio / WhatsApp arrival now carry a delivery identity?

Host `hetzner-dsm`, worktree `/root/wayland-twilio-whatsapp-identity`, commit
`30bdb74ebe9f1533449eed6c61d71a80f18efcdf`.

```
cargo test -p wcore-channels-registry --test identity_at_the_sink -- --ignored --nocapture
```

Real chain, nothing stubbed and nothing reimplemented:

```
production factory -> real adapter -> real HTTP -> scripts/f24-sink.mjs (own OS process)
  -> arrivals journal -> classifyRepeats() IMPORTED from scripts/f24-journey.mjs
```

## Verbatim transcript

```text
running 1 test
SINK url=http://127.0.0.1:38951 journal=/tmp/.tmp0f8nnW/arrivals.jsonl

arrivals=12 arms=3
arm         endpoint            arr  replay  recur  indet  unid  verdict
----------- ------------------- ---- ------- ------ ------ ----- --------------------
A-UNKEYED   twilio.messages        2       0      0      1     2 NOT-PROVEN
A-UNKEYED   whatsapp.messages      2       0      0      1     2 NOT-PROVEN
B-KEYED     twilio.messages        2       0      1      0     0 RECURRENCE
B-KEYED     whatsapp.messages      2       0      1      0     0 RECURRENCE
C-REPLAY    twilio.messages        2       1      0      0     0 EXACTLY-ONCE-VIOLATED
C-REPLAY    whatsapp.messages      2       1      0      0     0 EXACTLY-ONCE-VIOLATED

test twilio_and_whatsapp_arrivals_now_carry_a_delivery_identity ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
```

`WLRC=0`, read back from the log file rather than from the ssh exit status — see
the instrument note at the bottom, which caught a real false green in this lane.

## Reading it

**The gate CAN fail.** `A-UNKEYED` is the pre-change state reproduced on the
post-change binary: two arrivals of one body, neither carrying an identity,
`unidentified=2`, the repeat graded `indeterminate=1`, verdict `NOT-PROVEN`. This
is the control that matters, because without it "the numbers improved" is equally
explained by a harness that got kinder. It did not; the same binary still
produces the old verdict when the send carries no id.

**The gate CAN pass, and for these two adapters that state was previously
unreachable.** `B-KEYED` sends the same body under two distinct delivery ids —
the ordinary recurring-trigger case — and gets `unidentified=0`,
`indeterminate=0`, `recurrences=1`, verdict `RECURRENCE`. Before this lane, a
`twilio.messages` or `whatsapp.messages` repeat could not reach this verdict *in
principle*, no matter how well the product behaved, because the arrival carried
nothing to judge it by.

**Identity did not make the gate blind.** `C-REPLAY` sends one body twice under
ONE delivery id and is still caught: `replays=1`, verdict
`EXACTLY-ONCE-VIOLATED`. This is the arm that matters most for trust. The cheap
way for this change to go wrong was for every repeat to start classifying as a
benign recurrence, which would have looked like a large improvement and been a
regression. It does not.

Both endpoints behave identically across all three arms, which also rules out the
change having landed on only one of the two adapters.

## Scope of the claim — read before quoting the improvement

This measures **attributability at a destination that records what we sent.** It
is not a measurement of Twilio's or Meta's behaviour, and it must never be cited
as one. `supports_outbound_idempotency()` remains `false` on both adapters. The
run that would settle the platform question is
`crates/wcore-channels-registry/tests/live_twilio_whatsapp_identity.rs`, which is
credential-gated on **`WL_LIVE_TWILIO_HOME` + `WL_LIVE_TWILIO_TO`** and
**`WL_LIVE_WHATSAPP_HOME` + `WL_LIVE_WHATSAPP_TO`**. We hold neither credential.
**Two live cells remain unrun and a skip is not a pass.**

It also does not run the journey's 17 steps. The kill-and-recover leg is
untouched by this change; the column that changed is the identity column, and
that is what is measured here.

## Two independent causes had to be fixed, and only this test could see both

The adapter unit tests prove the header and the JSON field leave the process.
They would all have gone green while this number moved by **zero**, because
`scripts/f24-sink.mjs` hardcoded `idempotency_key: null` on both endpoints
(`:176` and `:193` at base). Arm B's `unidentified=0` is the only assertion in the
lane that spans both, and it deliberately cannot say which of the two was
missing — only that neither is.

## Instrument note — a false green caught in this lane

The first run of this test failed to compile (`WLRC=101`, ten `E0277` errors) and
the background-task harness reported **exit code 0**. That is LANE-BRIEF §2a's
warning in the wild: the ssh wrapper's status is not the remote command's. The
failure was found only by reading the log file, which is why every number above
is quoted from `/tmp/twid-arms2.log` on the build host and not from an exit code.
