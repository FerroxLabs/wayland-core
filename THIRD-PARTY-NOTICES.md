# Third-Party Notices

wayland-core includes code ported from third-party open-source projects. Their
copyright notices and license texts are reproduced below, as their licenses
require.

---

## Nous Research — Hermes (MIT)

Portions of wayland-core are ported from Nous Research's Hermes agent
(MIT-licensed). The following modules contain code that derives from it:

- `crates/wcore-tools/src/todo.rs` — todo tool
- `crates/wcore-tools/src/yuanbao_tools.rs` — Yuanbao tools
- `crates/wcore-tools/src/discord_tool.rs` — Discord tool
- `crates/wcore-tools/src/send_message.rs` — send-message tool
- `crates/wcore-tools/src/session_search.rs` — session-search tool
- `crates/wcore-tools/src/homeassistant_tool.rs` — Home Assistant tool
- `crates/wcore-tools/src/transcription_tools.rs` — transcription tools
- `crates/wcore-tools/src/vision_tools.rs` — vision tools
- `crates/wcore-types/src/cache_tier.rs` — cache-tier definitions

```
MIT License

Copyright (c) Nous Research

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## OpenClaw (MIT)

Three parts of wayland-core derive from OpenClaw. The scope of each is stated
precisely, because it is narrow and because an imprecise notice is its own
problem. See `.planning/PROVENANCE-COMPARISON.md` for the per-site evidence.

- **`crates/wcore-providers/src/failover.rs` — the `FailoverReason` taxonomy
  only.** Ten of the eleven variant names, their wire strings, and their
  order reproduce OpenClaw's TypeScript string-union. The match is deliberate,
  so that logs and telemetry correlate across the two implementations. No
  surrounding code derives from OpenClaw: the classification logic, the error
  chain, and the builder are unrelated to theirs.

- **`crates/wcore-channel-imessage`** and **`crates/wcore-channel-msteams`.**
  These Rust crates were written from the Wayland desktop application's
  TypeScript `ImessagePlugin` and `MsTeamsPlugin`, which are themselves
  documented derivations of OpenClaw's `extensions/imessage` and
  `extensions/msteams`, and which carry this attribution in their own source.
  Whether OpenClaw expression survives into the Rust has not been established
  either way; the notice is kept because the upstream derivation is documented
  and self-attributed.

Other modules once carried an OpenClaw attribution in error and no longer do.
Those were independently written; three of them described OpenClaw's behaviour
incorrectly, which is what prompted the review.

```
MIT License

Copyright (c) 2025 Peter Steinberger

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
