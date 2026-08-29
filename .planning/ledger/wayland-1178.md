---
issue: 1178
repo: FerroxLabs/wayland
title: "A base_url with the conventional /v1 suffix builds /v1/v1/chat/completions and 404s"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A base URL carrying the conventional /v1 suffix either reaches the endpoint or fails with an error naming the doubled path"
    state: not-met
    owner: core
    note: "silently composing /v1/v1/ and surfacing a bare 404 is the specific behaviour the issue says has to stop"
  - id: c2
    text: "A test covers the bare-root spelling, http://host:port"
    state: not-met
    owner: core
  - id: c3
    text: "A test covers the /v1-suffixed spelling"
    state: not-met
    owner: core
  - id: c4
    text: "A test covers the trailing-slash spelling"
    state: not-met
    owner: core
  - id: c5
    text: "Configs that already specify the bare root keep working unchanged"
    state: not-met
    owner: core
    note: "the blast radius is why this was not folded into #1173; many working configs depend on today's composition"
---

`openai_defaults()` sets `api_path` to `/v1/chat/completions` and appends it to
the configured base URL. The spelling every OpenAI-compatible server prints in
its own docs — `http://127.0.0.1:11434/v1` — therefore builds
`http://127.0.0.1:11434/v1/v1/chat/completions` and 404s. Only the bare root
works, and nothing in the failure points at the doubled path, so the natural
read is that the server is incompatible rather than that four characters are
surplus.

This affects every OpenAI-compat endpoint, not only local ones. It was found
while fixing #1173 and deliberately left out of that lane, whose scope was the
credential gate.

The three spellings are three criteria because the schema takes one piece of
evidence per criterion, and because the entire defect is that two of the three
look identical to a user and only one works. c5 records the constraint that
makes this more than a one-line change.
