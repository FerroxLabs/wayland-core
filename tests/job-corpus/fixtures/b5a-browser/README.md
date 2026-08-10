# B-5a fixture — place the order on the depot tool (browser)

**Row:** B-5, browser half. Separate from the native-app half; graded and
reported separately.

**User story.** The depot's stock tool is an internal web app with no API. To
raise tonight's order somebody has to sign in, load the catalogue, drag the
quantity to 62, tick expedite and press the button. It bills per order and has
no cancel, so doing it twice is worse than not doing it.

## What makes this a browser job rather than an HTTP job

* The part ids do not exist in the served HTML. They arrive only after
  `Load catalogue` runs in the page.
* The order token is derived from where the slider handle actually ends up on
  screen (`getBoundingClientRect`), so a quantity that was never put there
  cannot produce a valid order.
* The server refuses a submission unless the page reported a real drag
  (pointerdown, three or more pointermoves, pointerup) or keyboard operation of
  the slider, on the same session.

Residual risk, stated plainly: this raises the floor, it does not make forgery
impossible. Somebody who reads `app.js` could reproduce the sequence over plain
HTTP. The grader therefore also records the user-agent and `Sec-Fetch-*`
headers on the accepted order and notes their absence. It notes; it does not
fail on them. See the key.

## Procedure

```
python3 site_server.py --port $PORT --ledger $EVID/site-ledger.jsonl
```

Give the job `prompt.txt` with `PORT` substituted. Run with
`env -u API_KEY -u FLUX_API_KEY`. When it finishes, copy the ledger into
`$EVID/` and write `run.json`:

```json
{"surface": "browser", "platform_claimed": true, "surface_unavailable": null}
```

If the product could not drive a browser at all, put the reason in
`surface_unavailable` — that is a FAIL, recorded honestly, not an excuse.
Set `platform_claimed` to `false` only if browser control is genuinely not
claimed on this platform; then the row is N/A.

Grade with `graders/grade_b5.py --fixture browser --evidence $EVID`.
