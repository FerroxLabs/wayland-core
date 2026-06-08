---
name: ijfw-debug
description: "Root-cause analysis with hypothesis tracking. Trigger: 'debug', 'broken', 'not working', 'fix this bug', /debug"
---

## Step 1 -- Reproduce
State the failure in one line: what was expected vs. what happened.
Confirm reproducible before proceeding. If intermittent, note conditions.

## Step 2 -- Check recent changes
Call `ijfw_memory_recall` with the symptom. Scan `.ijfw/memory/` for decisions or
changes from the past 7 days that touch the affected area.

If a recent change correlates with the regression, offer revert first:
> `Regression likely from <change> on <date>. Revert first? (y/n)`

## Step 3 -- Isolate
- Narrow to the smallest reproducing case (file, function, line range).
- Determine if failure is input-dependent, environment-dependent, or logic-dependent.
- Read only the specific lines relevant to the hypothesis. No full-file reads.

## Step 4 -- Hypothesize
List hypotheses ranked by likelihood:
```
H1 -- <most likely cause> -- evidence: <why>
H2 -- <next candidate>    -- evidence: <why>
H3 -- <edge case>         -- evidence: <why>
```
Confirm H1 before testing H2.

## Step 5 -- Fix and Verify
Apply the minimal change that addresses the root cause.
Do not fix adjacent issues -- log them as follow-ups.
Run tests/linter after every fix.
Confirm the original symptom is gone and no adjacent regression introduced.
Store result: `ijfw_memory_store: <what broke>, <root cause>, <fix applied>`

## Step 6 -- Two-strikes session reset
If two attempts at root-cause fixes both fail to clear the original symptom, stop. Do not try a third on the same hypothesis tree. Summarize in three lines: what you tried, what each attempt revealed, what you now believe is true. Then ask the user:

> `Two attempts didn't land it. Recommend resetting this session and starting fresh with: "<sharpened prompt>". Accumulated failed context degrades the next attempt; a fresh session with a tighter brief usually clears it on the first try.`

Capture the summary in `ijfw_memory_store` so the next session inherits the lessons without inheriting the noise.

## Output format
```
SYMPTOM:     <one line>
ROOT CAUSE:  <H1 confirmed or revised>
FIX:         <what changed + file:line>
VERIFIED:    yes / needs more testing
FOLLOW-UPS:  <any adjacent issues deferred>
```
