# Changelog

## [0.13.5](https://github.com/FerroxLabs/wayland-core/compare/v0.13.4...v0.13.5) (2026-08-22)

**Release highlights.** Twenty-one fixes with one subject: a guard that exists is
not a guard that runs. Most of what changed here was already written, already
tested, and already correct — and did not reach the path you were actually on.
The rate limiter that stops an agent talking to itself forever was never
consulted on the desktop. The session-saving repair covered one failure arm out
of six, and the arm it missed was the common one. The skill containment guard
was workspace-scoped, so it never reached the directory the escape used. In each
case the unit test passed, because the unit was fine. The second subject is
silence: several things the product knew, it did not say. No breaking changes;
the protocol contract stays at minor 16.

**A retry that succeeded no longer fails your turn.** This is the most serious
one. When a provider errored and the retry then worked — the answer generated,
the content returned — the turn could still be handed back as a journal
authority error, because the failed attempt was never marked terminal and the
commit path checks that with the same predicate the failure path uses. So a
recovery was rendered as a failure. There are now four settle points where there
were two, covering the retry arm, the retry-exhausted exit, and the
compact-and-resend arm, and each has its own test rather than sharing one.

**The product stops going quiet on you.** Two silences, and they were not the
same bug. A request whose connect never completes — a blackholed endpoint, or a
typo in your `base_url` — used to produce thirty seconds of nothing at all
before the first retry line appeared, because the notice was armed inside the
stream loop and every provider builds its channel only after the connect
returns 2xx. There was not even a channel to notice on. A request that *does*
connect and then produces no bytes had a channel, but could stay quiet for the
full five-minute read timeout. Both are announced now, on your surface rather
than a log you do not have enabled. The threshold moved from thirty seconds to
fifteen, and had to: thirty was exactly the connect deadline, so the notice was
scheduled for the same instant as the failure it exists to precede and could
never arrive first. It is only a notice — measured against a blackholed
endpoint, the retry sequence and total wall time are unchanged within about
fifty milliseconds. Relatedly, a connect deadline that expired was being
classified as a generic connection failure rather than a timeout, which routed
it into a fifteen-minute outage budget it was never meant to have. That one was
live, not theoretical: the same failure renders two ways, and the spelling that
contains no "timed out" anywhere accounted for 281 of 300 measured connect
timeouts — so a typo in your `base_url` bought the full window about fifteen
times in sixteen, and failed fast the other time. That is the nine-hundred-second
hang and the thirty-six-send storm seen in testing. It fails fast now, on all
three spellings including the Windows one, with a control proving genuine
refusals and DNS failures keep their own class.

**The stream retry budget is yours to set.** It was a constant compiled into a
function. It is now configurable with a ceiling, and if you ask for more than the
ceiling the product clamps it *and tells you which number it is actually using* —
an unreported clamp is a quieter lie than the one it replaces. The default is
unchanged, and there is a test asserting the default is unchanged, because a
silent change to everyone's retry behaviour would be worse than the limitation.

**Rate limiting reaches the desktop.** Under host-delegated sending — how the
desktop app sends — the tool kept a transport that consulted no limiter at all,
so agent-to-agent reply loops were entirely unthrottled there. The throttle now
reaches the model as an error result rather than only a log line, on both
delivering transports. The human and operator paths remain unthrottled, which is
deliberate and pinned by its own test.

**Skill output has a destination, and artifact writes go through the jail.**
These two shipped together because fixing either alone relocates the problem
rather than closing it: closing the output convention alone moves the escape to
the repo-control surface through frontmatter, and closing the write path alone
leaves the guard scoped to the workspace so it never reaches the global config
directory. One thing that shipped after: the same feature rendered that
directory two ways. The header telling a skill where to write normalised the
path to forward slashes and the `{output_dir}` token did not, so on Windows a
skill body could carry `F:\Temp\...\containment/brief.html` — mixed separators
in model-facing prose the model then puts into a shell command, where a
backslash is an escape character. Both sites now go through one normaliser.

**The workspace boundary walk can be interrupted, and it is faster.** The
per-execution secret-deny walk sat outside both cancellation points, so neither
Ctrl-C nor your own `timeout` parameter bounded it — the parameter did not do
what it said. Both call sites are fixed. The walk itself is now parallel, which
on a large contained workspace is roughly five times faster, with the resulting
deny set proven byte-identical to the serial one across symlink loops,
execute-only directories, and nested roots. Worth stating plainly: an earlier
report of a 76-second stall was a Windows measurement for a path already fixed
in 0.13.1. The real cost on Linux was about nine tenths of a second warm, and
one and four tenths cold, on a ninety-thousand-entry tree. The defect was that
you could not interrupt it, not that it was slow.

**Browser DNS resolution is checked, and the claim about it is honest.** The
resolution gate did not exist — the browser crate never resolved a hostname
anywhere — so it was built. Static DNS-based SSRF is now closed. What is *not*
closed is intra-navigation rebinding against a zero-TTL record, because the
browser resolves in its own process; the operator hint and the README now say
that instead of implying otherwise. A pin-first-answer cache was written and
then removed after measuring real DNS: across eleven re-queries over about a
minute, the answer set for `s3.amazonaws.com` was completely disjoint from its
first answer eleven times out of eleven, and `cdn.jsdelivr.net` eight times out
of eleven, against a `www.wikipedia.org` control that stayed stable on all
eleven. Pinning would have refused ordinary hosts for the rest of a session
while buying nothing.

**Smaller, but each one is a thing that lied to you.** Pending approvals are now
cancelled when the host command stream ends, instead of stalling until a
twenty-four-hour token expiry. `doctor` honours `--profile` and `--project-dir`
rather than discarding them and reporting on a configuration you did not ask
about. A turn that ended because it hit a length limit, a turn limit, or an
error is now distinguishable from one that ended cleanly. A chain exhausted on a
server error keeps its failure class instead of being flattened into a
connection error and granted a retry window the engine deliberately withholds
from server errors.

**Esc no longer ends the conversation.** Pressing Esc during an in-flight turn —
the key the in-turn keybar advertises as `interrupt` — stopped the stream and
then left that conversation permanently unusable. The composer still took text
and the cursor still blinked, but every further message was refused, and the
recovery paths formed a closed loop: cancel pointed at resume, resume pointed at
reconcile, and reconcile said only the engine could. Twelve times out of twelve
in testing, and 0.13.4 behaves the same way. The cause sat above every one of
those refusals. The interrupt path wrote no terminal receipt of any kind — no
stream end, no provider attempt outcome, no turn commit — where a Ctrl-C or a
clean finish wrote two of each, read back through the same journal reader. The
turn was left at an outcome nothing could resolve, so each refusal was a correct
guard reacting to a hole punched above it. The fix does not open that guard. A
request that left this machine and never came back genuinely has no knowable
outcome, so claiming it succeeded, failed, or never started stays refused. What
was missing is the fourth option, and it is here now: the turn can be abandoned,
recording the outcome as unknown — including that the provider may have served
the request in full, in part, or not at all, and that anything it charged is not
accounted for — alongside the digest of exactly the bytes that were captured. It
is written at interrupt time by the live engine, the only party that can, so
there is no wedge left to recover from afterwards. The way out is named on every
surface the refusal reaches: `/recover abandon` in the terminal UI, and
relaunching the session with `--resume` anywhere else, including the JSON stream
host, whose recovery vocabulary never had an `abandon` verb to offer.

## [0.13.4](https://github.com/FerroxLabs/wayland-core/compare/v0.13.3...v0.13.4) (2026-08-21)

**Release highlights.** One new capability and four fixes, and they share a
subject: where the boundary of your workspace actually is, and whether the
product tells you the truth about it. The boundary was wrong in both directions.
Things you could reach were reported as out of bounds — a denied read came back
as emptiness, and a path with a space in it was called outside your workspace
when it never was. Things you should not have been able to reach were writable:
`.git` hooks and the agent's own skill files. Meanwhile the one honest refusal,
crossing the workspace edge, was the least useful thing it could be, because a
refusal is not a question. This release makes the boundary correct, makes the
refusals accurate, and in the largest case replaces the refusal with a question.
No breaking changes.

**You can grant a folder instead of being refused.** Reading a file outside the
workspace used to end the same way every time: a denial, and a suggestion to
restart under a different profile. There is now a fourth approval scope,
`AlwaysPath`, that grants a directory for the rest of the session, and the
boundary is checked *before* the read rather than discovered by failing it — so
you are asked about a folder you are about to cross, instead of being told after
the fact that you should not have. Three pieces make it work together and are
released together because none is useful alone: the grant itself, the pre-flight
boundary classifier that knows a read is about to leave the workspace, and a
`render_artifact` path that hands content to the host without handing over any
filesystem authority. The protocol contract goes from minor 14 to 16, which is
additive: an existing host keeps working untouched.

One limit is worth stating plainly rather than discovering. `Once` on a boundary
card still cannot work — the authority plumbing does not support it, Enter is
still bound to it, and the documentation that claimed otherwise has been
corrected rather than the claim being made true. It is tracked, and not fixed
here.

A refused folder grant now says so on every session shape. The approval itself
still stands for the call you approved — that is deliberate, and a refusal of
the standing grant must never turn a yes into a no — but the grant failing to
take is no longer silent. It was already announced on sessions where the agent
installed the workspace policy itself. It is now announced on the ones where
another layer installed it first, which previously had no reporting channel at
all and so refused silently on every surface, stderr included. No authority
changed: the reporting layer forwards the policy's answer verbatim, and a
session that was refused before is refused now, and told why.

**A path with a space in it is one path.** The advisory that explains a denied
read split tool output on whitespace, so every macOS path under
`Application Support` broke into two tokens, both of which then classified as
ungranted. The result was a confident, specific, entirely invented message
telling you the file was outside every root granted to your workspace — about
files that were never out of reach. Because every Wayland desktop workspace lives
under `Application Support`, this fired on the common case rather than an exotic
one, and it cost two lanes and one user a day chasing a sandbox boundary problem
that the error message had made up. Space-joined runs are now reassembled with
the filesystem as the arbiter, and the test that matters is the control: a file
that genuinely *is* outside your granted roots, also with a space in its name,
still has to trigger the advisory.

**The file tools can read your repository's control surface but never write
it.** `.git` and `.wayland-core` were writable by the in-process `Write` and
`Edit` tools in the trusted-local profile. Two concrete consequences, neither
theoretical: a written `.git/hooks/pre-commit` is arbitrary code execution on
your next commit, and a rewritten `.wayland-core/skills/*/SKILL.md` is arbitrary
instruction injection into your next session — the agent editing its own
standing orders. Both are now refused, in every profile, with an error that says
to use the Bash tool and a real git command if the write is genuinely intended.
Reads are untouched: this surface is write-denied, never read-denied.

There is also an opt-in `[security] require_vcs_for_writes`, OFF by default,
which keeps the strict profile in a workspace that is not under version control
— on the reasoning that an unversioned workspace has no undo.

**A denied file no longer reads as an empty one.** A masked read fails loudly on
its own, but a compound command lets the shell swallow it — `cat secret.pem; echo
rc=$?`, `cat x || true`, `cat x 2>/dev/null` — and what reaches the agent is
success plus no bytes. That difference is acted on: a populated file gets
reported as empty, a step is treated as having succeeded on no data, and a file
the agent was never allowed to see can be overwritten in the belief that it was
blank. The denial is now annotated on the success path too, by scanning the
command rather than the output, since the empty output is the whole problem.
Containment is unchanged, and the tests assert the secret's bytes never appear
and the host file is byte-identical afterwards.

**A command that produces too much output keeps what fits.** The Docker execution
backend answered an over-cap command by discarding everything and returning an
error, so the same command behaved differently depending on a backend you did not
choose and mostly cannot see. It now grants what fits, appends the same
truncation marker every other backend uses, and stops the container. Measured on
a live daemon against a flood that never reaches EOF: 0 bytes before, 8,388,930
after. A separate defect surfaced while fixing it and is fixed too — a non-zero
container exit was being mapped to a generic I/O error that threw away both the
exit status and every byte the command had already written.

**Documentation for three shipped subsystems that had none.** Crucible (the
cross-provider council), isolated profiles, and cost governance were all
shippable and all undocumented. Every claim in the new pages was checked against
the code before publishing rather than trusted from the draft.

## [0.13.3](https://github.com/FerroxLabs/wayland-core/compare/v0.13.2...v0.13.3) (2026-08-19)

**Release highlights.** Two fixes, both about the product telling the truth. A
privacy notice that was being written somewhere nobody reads, and an error that
had been hiding its own cause well enough to survive several release trains. No
breaking changes.

**You are now told when your searches leave for a third party.** The keyless
default web-search backend sends queries to `parallel.ai`, and the notice saying
so was emitted into the log file rather than to your terminal, because with
`RUST_LOG` unset only errors reach stderr. You had never seen it. That mattered
more from 0.13.2 onward, since that release is what made the keyless default
actually reach the network for the first time — before it, every request was
refused by the egress policy and quietly served by DuckDuckGo instead. The notice
now prints where you are, once, and every failure path errs towards showing it
rather than skipping it.

**The workspace admission error stops hiding where it came from.** A lock-file
`ENOENT` was deliberately passed through as a raw OS error so one caller could
read "no lock file" as "nobody holds the lease". A second caller shared that
path, where the condition is neither expected nor handled, so it escaped as
`io: No such file or directory (os error 2)` with no path and no probe — and
because it originates in the sandbox layer, it slipped past every site in the
swarm layer that names itself. That is the whole reason this defect stayed
unmeasurable: its CI output contained nothing to look at. The acquiring path now
names the lock file it could not take and says its parent directory is missing.
The probing path is deliberately untouched, and a test pins that: changing it too
would make every transaction read as inactive and stop capacity accounting
counting reservations at all, which is a considerably worse bug than the one
being fixed.

## [0.13.2](https://github.com/FerroxLabs/wayland-core/compare/v0.13.1...v0.13.2) (2026-08-18)

**Release highlights.** Web search has not actually worked since June, and this
release is mostly about things that were failing silently. A shipped default that
could never reach the network, a config typo that sent your API key to the vendor
you were trying to avoid, a command that produced 20 MB and handed back 129 bytes,
and a host integration that answered malformed input with nothing at all. Plus a
security bump for a dependency advisory published the day before. No breaking
changes.

**Web search reaches the network again.** Three of the six built-in search
backends — including the **keyless default** — called hosts that were not on the
product's own egress allowlist, so every request was refused before it left the
process. On a default install the primary backend could never succeed, and
`EXA_API_KEY` and `FIRECRAWL_API_KEY` were inert no matter what you set. Because
the chain degraded silently, you saw only the DuckDuckGo outcome and were advised
to set a Brave key — for a backend that was never involved. The hosts are now
allowlisted, and a result served by the fallback carries a `degraded_from` note
naming the backend that was skipped and why.

**A misplaced `base_url` no longer sends your key to the vendor.** Unknown and
misnested config keys were detected correctly — that has worked since 0.12 — but
the warning went to `tracing::warn!`, and with `RUST_LOG` unset that reaches a log
file rather than your terminal. So a top-level `base_url` (the correct spelling is
`[providers.anthropic] base_url`) was silently ignored while your prompt and real
credentials went to the real endpoint, which is the precise outcome you were
configuring to prevent. Unrecognised keys are now named on stderr, with a targeted
hint for `base_url`.

**Large command output is truncated, not discarded.** Exceeding the 8 MiB buffered
output cap returned an error and dropped everything already read: 20 MB in, 129
bytes out. You now keep the first 8 MiB plus a marker stating that the command was
stopped and did not run to completion. Worth knowing why it is the head and not the
tail: crossing the cap is also the trip wire that terminates the child, so a child
stopped at the cap has no tail — its last bytes are where reading stopped, not where
the command ended.

**The host protocol answers instead of going quiet.** A malformed command over the
JSON stream produced no wire response at all, which is indistinguishable from a hang;
each refused line now emits one `error` frame naming the offending type and the
reason. Separately, closing the host's stdin while a tool approval was parked left it
waiting out a 300-second timeout plus a sweep — about 330 seconds. It now resolves in
about a millisecond, failing closed, with a reason distinct from a genuine timeout.

**Swarm dispatch no longer deadlocks against its own lock.** A cleanup dropped inside
the registration critical section re-acquired a lock the same thread already held —
`flock` ownership is per open file description, so a second acquisition from one
process blocks exactly like a foreign one. That was a hang rather than an error.

**Security.** `h2` moves to 0.4.16 for RUSTSEC-2026-0258 (unbounded empty DATA
frames), published 2026-08-17. `h2` sits under hyper/reqwest and therefore on the
path of every outbound provider request.

**Also.** Every CI job backing a required status check now declares a timeout; one of
them previously inherited a six-hour default and blocked the repository twice in a
day. Materialization failures in swarm admission now name the probe and the path
instead of a bare `No such file or directory`.

**Known issue.** The macOS-only workspace-admission flake (`#1025`) is **not fixed**
in this release. It is now diagnosable — the failure names its site rather than
surfacing as a bare ENOENT — and the reproduction has been narrowed on hosted macOS
runners, but the cause is still open. The invariant it guards holds: capacity is
never overbooked.

## [0.13.1](https://github.com/FerroxLabs/wayland-core/compare/v0.13.0...v0.13.1) (2026-08-18)

**Release highlights.** A fast follow-up to v0.13.0, and most of it is Windows.
The command-execution cluster that users have been reporting since 0.12 —
"Bash not working", "echo times out", "sandbox child timed out" — turned out to
be two separate defects with one shared symptom. Both are fixed, and both have
now been exercised on real Windows hardware — for one of them we reproduced the
exact failing bytes and watched the fix remove them. Windows binaries are also
Authenticode-signed for the first time. No breaking changes, and no barrier to
rolling back.

**Windows commands stop timing out.** Every `Bash` invocation computed an
OS-level read-deny list by walking the entire workspace with no pruning, and the
default Windows backend then discarded the result. The cost was paid on every
command and scaled with the size of the tree: measured on a real profile, one
`echo` took **39,278 ms cold; it now takes 349 ms**. A clean temp directory hid
it completely, which is why it survived so long — the reproduction only appears
against a real checkout. The deny list is now computed only for a backend that
actually enforces it, read off the same backend handle that runs the command, so
it fails safe. Re-measured live on real Windows hardware: the walk itself costs
15,418 ms on a large real tree and 0 ms once skipped. Note the corollary — on a
repo-sized checkout the same walk costs about 79 ms, so if you re-test this in a
small tree or a temp directory you will correctly see no difference at all.
This is a latency fix, not a containment fix: `sandbox status` on
the Windows default still reports that it does not confine the filesystem, which
is accurate. Addresses FerroxLabs/wayland#892, #912, #918 and the core half of
#921.

**Windows stops corrupting command output.** `cmd /c` was invoked without `/S`,
so it took the quote-preserving branch and a nested `cmd /c echo …` came back
with a stray trailing double-quote — a single wrong byte on stdout that the
model then reasons over. `/S` is now passed from one shared prefix helper.
This is now confirmed against a running `cmd.exe` on Windows 11 build 26200,
not just reasoned from `cmd /?`: driving the same production code path with and
without `/S`, the payload `cmd /c echo NESTED` returns the bytes
`4e 45 53 54 45 44 0d 0a` with the fix and `4e 45 53 54 45 44 22 0d 0a` without
it. The stray `0x22` is real, and `/S` is what removes it. Payloads with no
nested `cmd` are byte-identical in both arms, so this is the switch and not an
output trim.

**Windows binaries are now signed.** This project has never shipped an
Authenticode signature. The signed release manifest and Sigstore provenance are
both real, but Windows reads neither — SmartScreen warned on every
`wayland-core.exe` we have published, and Smart App Control blocked it outright.
Release builds on both Windows targets are now Authenticode-signed through OIDC
federation, with no long-lived signing credential stored in the repository.

**Approval prompts fail closed.** An EOF on the approval prompt — Ctrl-D, a
closed stdin, an ended pipe — was read as approval, because the empty-string arm
meant yes. It is now a denial. Approval tokens are scrubbed centrally rather
than travelling on the tool-result wire, and an `Always` egress grant on the ACP
bridge no longer collapses to a single use. In the TUI, `[a] always for this
tool` is written when you grant it and replayed at launch, so it survives a
restart; the `[a] always for <prefix>` shell form and the desktop-host path are
deliberately unchanged.

**Credentials.** Multiple named accounts per provider already worked — secure
storage of them did not. One provider mapped to one credential slot, and an
alias with no key of its own inherited the builtin's, so a second account was
silently billed to the first account's key. Every non-builtin selection now gets
its own slot, resolved above the inline config value, and `auth add/list/remove`
take an account id. Single-account resolution is byte-for-byte unchanged. The
`${cred:KEY}` rail now also covers MCP stdio server environments, so a stdio
server can be given a secret by reference instead of by value.

**Browser.** Denial messages name the config paths the loader actually resolves,
instead of a setting that never existed, and loopback denials carry remediation
at all. The message no longer claims DNS-rebinding protection the code does not
implement — that enforcement is still missing and is tracked separately; the
honest message shipped without waiting for it. A project config that sets only a
loopback grant no longer has that grant silently dropped by the config merge,
and a project that sets an origin list no longer drops the operator's grant.
Opt-in Camoufox binary provisioning is wired: off by default, and it refuses to
fetch without an operator-pinned SHA-256.

**Providers.** A 5xx whose body names a permanent error — auth, billing, model
not found — is no longer retried through the full backoff ladder. 503 and 529
are never overridden, and a body naming a transient condition stays retryable.
One predicate governs both the failover classifier and the retry decision, so
they cannot disagree about the same response.

**Web search.** A DuckDuckGo anti-bot challenge answers HTTP 202 with no result
markup, which read as success and produced an empty parse — reported to users as
a change in their HTML format. Results are now parsed first and the challenge
consulted only when nothing was served, anchored to attributes the echoed query
cannot forge.

**Matrix.** A 403 refusal of a single redaction by an under-privileged bot was
classified as a dead credential and latched, marking the channel permanently
unauthenticated while every later send kept succeeding. 401 and 403 are now
separate questions, and an expired access token renews instead of killing the
adapter.

**macOS.** Establishing process-tree containment no longer fails when the child
has already exited. Darwin reports ESRCH for a process that is gone, so a
subprocess that ran to completion — routine for a fast `git config` on a loaded
machine — surfaced as `failed to establish process-tree containment: No such
process`. A root that is gone and took its process group with it is now
correctly treated as nothing to contain; a corpse that left a descendant behind
still gets a real sentinel, and the recycled-pid refusal is unchanged.
Screen-Recording and Accessibility permissions can now be requested up front
with the new `--request-permissions` flag, which is the only path allowed to
raise a consent dialog — ordinary agent runs still only probe, so a permission
sheet can never surprise you mid-task.

**MCP.** Skills and hooks provided by deferred-config MCP servers are late-bound
instead of missed.

**Host contract.** `schema_digest` is unchanged — no capability, event or
wire-shape moved. `source_inputs_digest` and `fixture_digest` both changed;
hosts that pin those two must re-pin.

## [0.13.0](https://github.com/FerroxLabs/wayland-core/compare/v0.12.26...v0.13.0) (2026-08-13)

**Release highlights.** 255 commits (3 `feat`, 140 `fix`, 64 `test`, 20 `docs`)
across 287 files. No breaking changes. This is the release that closes the 0.12
line: **58 days and 27 stable point releases** from v0.12.0 on 2026-06-16 to
here. It could have shipped as 0.12.27 and deliberately did not, because the
shape of the work is different. v0.12 built the surface; v0.13.0 is a cycle
spent attacking it. Almost everything below was found by us, not reported.

**MCP tool discovery works again.** At `compaction = "full"` a `ToolSearch`
catalogue was run through the same line-folding heuristic as build logs.
Pretty-printed JSON is full of legitimately similar lines, so a five-tool
catalogue collapsed from 27 lines to 5 with **zero of five tool names
surviving** — a model driving a 101-tool MCP server could not name one of its
tools, and correctly refused to guess. The invisible half: the engine parses
that same string to decide what to force-admit into `tools[]`, so hydration
recorded nothing, the tool never became callable, and every repeat search
returned a byte-identical body. Closed three ways — structured output never
meets the fold, the similarity metric normalises by the longer line so a
3-character `{` cannot anchor a group and swallow its own object, and results
are no longer cut mid-object by a character-count truncation. A body that
arrives corrupted now says so instead of silently hydrating nothing.

**Auto-approved calls are announced.** Every call that skipped the approval
gate — force mode, an allow-listed tool, a command-scoped grant, or a tool just
granted `Always` — dispatched with nothing on the wire. The new
`call_announced` frame carries the same payload as `tool_request` for exactly
those calls, closing two unreported TUI defects: no tool card at all, and a
touched path that never reached the right-rail tree or `/rewind`.

**Unsaved work is not collateral.** The largest single family of fixes. The
shell can no longer discard it, an `Edit` cannot drop it as a side effect, and
a redirect that would truncate a file with unsaved changes is refused. The
guard now covers `rm` and `git`, keys on one path spelling, anchors every copy
it takes under a ref, and knows a merge in progress is not unsaved work.

**Locks release when they should.** The session journal's data-file lock was
leaking through `fork()`, blocking an agent behind its own child in 47.6% of
reopens under load. Corrected there and swept across the cron schedule lease,
the gateway pid lock, the eval candidate identity lock and snapshot publication.

**Saying only what is true.** `sandbox status` no longer claims filesystem
containment on Windows, where the Job Object does not provide it (measured, not
assumed). A tool description no longer names the flag that bypasses it. Startup
warnings are said once rather than every turn. Requests that never returned a
response are disclosed instead of vanishing.

**Also:** runs that need human contact they cannot reach freeze state and exit;
single-use DM pairing codes with an operator verb; MCP `tools/list_changed`
honoured mid-session; crash-interrupted sessions recoverable; `ready` guaranteed
first on the json-stream; a rejected API key no longer retried; Windows connect
failures classified by WSA code rather than error text.

**Host contract.** `minor` 13 → 14, `major` holds at 1, additive only. 23
commands / 60 events / 17 capabilities. Hosts pinning the descriptor must
re-pin `fixture_digest` and `source_inputs_digest`; `schema_digest` is
unchanged. Consuming `call_announced` is required, not optional — dropping it
through a default arm leaves the `tool_running` behind it without a matching
request.

**Verified.** Linux 14,656/14,656 · macOS 14,547/14,547 · Windows self-hosted
14,174/14,174, zero failures, plus six platform builds, the eval acceptance
gate and the browser end-to-end suite. 21 required checks green. Full notes in
[docs/releases/v0.13.0.md](docs/releases/v0.13.0.md).


## [0.12.26](https://github.com/FerroxLabs/wayland-core/compare/v0.12.25...v0.12.26) (2026-08-08)

**Release highlights.** The largest release in the project's history: 2,918
commits (250 `feat`, 629 `fix`, 519 `test`, 740 `docs`) across 5,506 files in
26 days.

Durable **Goals** that outlive the process, unified across all five loop
owners and controllable over the host protocol. A provider-neutral **execution
backend** contract with local, ssh, container and cloud reference backends —
the cloud one genuinely creates, runs and hibernates a machine. An operable
**gateway** with an exactly-once delivery ledger, a single-owner inbound lease
and re-sendable abandoned deliveries. **Ten channels** with declared native
actions and a cross-adapter conformance matrix. A persistent **code index**
(`wayland-core index`). **Backup, restore and rollback** with a write-ahead
journal and consistent live-SQLite capture. **Importers** for openclaw, grok
and gemini-cli. A queryable **cost ledger** with a daily spend ceiling. Memory
controls you own. Governed **skills** with atomic rollback. **Voice**
default-on for macOS and Windows.

Security: the credential deny-list is now enforced on every read tool, a
fail-closed credential ladder replaces the plaintext fallback entirely, and
sandbox bypass cannot be activated remotely. Windows took the largest single
share of the release, including **140 ms → 68 ms per op at 24-way**
concurrency. Releases ship a signed manifest, a deterministic SBOM, keyless
Sigstore provenance and a fail-closed updater.

Full notes: [docs/releases/v0.12.26.md](docs/releases/v0.12.26.md).
Desktop integrators: [docs/releases/v0.12.26-desktop-integration.md](docs/releases/v0.12.26-desktop-integration.md).

## [0.12.25](https://github.com/FerroxLabs/wayland-core/compare/v0.12.24...v0.12.25) (2026-07-13)

**Release highlights — Anvil Smart Loops.** The gated forge is on by default:
auto-detected gates, natural-language invocation via the `Forge` tool,
cheap-driver seat routing (incl. FluxRouter), an escalation valve that buys
exactly one frontier turn on a stall, and machine-stamped receipts where only
a passing gate earns `verified`. Hardened by a dual cross-vendor adversarial
audit; live-proven end to end.


### Features

* **acp:** additive persona-agent protocol types (agents/list, selector) ([#202](https://github.com/FerroxLabs/wayland-core/issues/202)) ([09dc167](https://github.com/FerroxLabs/wayland-core/commit/09dc167271656d40a91dfb69b45af5715c72b6f0))
* **acp:** agent roster trait + capability handshake ([afddf24](https://github.com/FerroxLabs/wayland-core/commit/afddf24f7c2e47c59a97e8ac027a7e92f0920a3e))
* **acp:** bind selected persona to the session engine (persona PR-4) ([#215](https://github.com/FerroxLabs/wayland-core/issues/215)) ([59e8676](https://github.com/FerroxLabs/wayland-core/commit/59e8676cf1a757a2602eef0a85291de1a5030fbe))
* **acp:** CliAgentRoster — trusted persona enumeration (persona PR-3) ([#213](https://github.com/FerroxLabs/wayland-core/issues/213)) ([01836ce](https://github.com/FerroxLabs/wayland-core/commit/01836cecee20316c45159054fb2cc9351af9c3cd))
* **acp:** defang forged host trust-tags in persona SOUL (PR-5) ([#218](https://github.com/FerroxLabs/wayland-core/issues/218)) ([4915c13](https://github.com/FerroxLabs/wayland-core/commit/4915c135510b0f4f525c873b6b2003bde87137d8))
* **acp:** profile supervisor router, one process per profile (PR-7) ([34d8676](https://github.com/FerroxLabs/wayland-core/commit/34d86766ae1ee2e631181edb42d0666a64f80393))
* **acp:** serve an isolated profile via `acp serve --profile` ([#216](https://github.com/FerroxLabs/wayland-core/issues/216)) ([661dd98](https://github.com/FerroxLabs/wayland-core/commit/661dd9827e35281b52e43caa2d3c943393f40cf1))
* **acp:** stamp per-turn turn_id on terminal Done/Error frames ([#787](https://github.com/FerroxLabs/wayland-core/issues/787)) ([#219](https://github.com/FerroxLabs/wayland-core/issues/219)) ([b11aba5](https://github.com/FerroxLabs/wayland-core/commit/b11aba5ab83e282584d5b653ad087cba67533697))
* **anvil:** A1 native gated-forge engine — live-proven /forge (A1.3–A1.6) ([#246](https://github.com/FerroxLabs/wayland-core/issues/246)) ([1f89ad4](https://github.com/FerroxLabs/wayland-core/commit/1f89ad42039ebde0ac80e278a73bfe2a740fb4ae))
* **anvil:** A1.1 engine skeleton + kill-switch + forge verb ([#230](https://github.com/FerroxLabs/wayland-core/issues/230)) ([9928c85](https://github.com/FerroxLabs/wayland-core/commit/9928c85fc3d9697b3ecaa71055f3c5e1361bce2f))
* **anvil:** A1.2 AnvilReceipt event + receipt trust boundary ([#231](https://github.com/FerroxLabs/wayland-core/issues/231)) ([ca71a88](https://github.com/FerroxLabs/wayland-core/commit/ca71a888c68b52e203e1249eaa741b839220b95d))
* **anvil:** gate closure/probe + cost ledger (A1.3+A1.4) ([#236](https://github.com/FerroxLabs/wayland-core/issues/236)) ([ea3bb1c](https://github.com/FerroxLabs/wayland-core/commit/ea3bb1c584b157e7e83d34db25d96e1136e9f584))
* **anvil:** Smart Loops layer — default-ON, auto-gate, seat routing, Forge tool, valve (A1.7–A1.10) ([#247](https://github.com/FerroxLabs/wayland-core/issues/247)) ([49b58e5](https://github.com/FerroxLabs/wayland-core/commit/49b58e54c4608460490007fa962fe4c1596f1bdd))
* **cli:** add migrate hermes importer for named profiles ([#228](https://github.com/FerroxLabs/wayland-core/issues/228)) ([#226](https://github.com/FerroxLabs/wayland-core/issues/226)) ([c5e9a8e](https://github.com/FerroxLabs/wayland-core/commit/c5e9a8e043b339baa879da2b0c61501a4ee106a0))
* **proving-ground:** M1 harness + provenance & detection tests ([#53](https://github.com/FerroxLabs/wayland-core/issues/53)) ([#228](https://github.com/FerroxLabs/wayland-core/issues/228)) ([363d918](https://github.com/FerroxLabs/wayland-core/commit/363d918adc080699ea445229d6f065686ed74244))


### Bug Fixes

* **agent:** /agent new aborts on non-TTY stdin instead of reading it ([#212](https://github.com/FerroxLabs/wayland-core/issues/212)) ([e93a879](https://github.com/FerroxLabs/wayland-core/commit/e93a879e2c01397701e2debb9c11f3492f9153f2))
* **bash:** recompute secret-deny per exec (post-bootstrap) ([#234](https://github.com/FerroxLabs/wayland-core/issues/234)) ([#241](https://github.com/FerroxLabs/wayland-core/issues/241)) ([3317601](https://github.com/FerroxLabs/wayland-core/commit/33176014d0c39efd72981216ea14de81847476e6))
* **bash:** refuse local-data network uploads to block exfil ([#673](https://github.com/FerroxLabs/wayland-core/issues/673)) ([#224](https://github.com/FerroxLabs/wayland-core/issues/224)) ([f60c25d](https://github.com/FerroxLabs/wayland-core/commit/f60c25d17bda1acf48325e64497c50b2609c927b))
* **creds:** migrate plaintext secrets into vault on unlock ([#183](https://github.com/FerroxLabs/wayland-core/issues/183)) ([#221](https://github.com/FerroxLabs/wayland-core/issues/221)) ([e84ee26](https://github.com/FerroxLabs/wayland-core/commit/e84ee261e6f9ec9e97e468ec223c5fa9299a1ca8))
* **engine:** don't persist unrecoverable over-ceiling session ([4a65215](https://github.com/FerroxLabs/wayland-core/commit/4a652150206952888d9b7290045cdbd65d7c397a))
* **exec:** cross-platform command execution — unbreak Windows ([#754](https://github.com/FerroxLabs/wayland-core/issues/754)) ([#207](https://github.com/FerroxLabs/wayland-core/issues/207)) ([2ead3d5](https://github.com/FerroxLabs/wayland-core/commit/2ead3d5d49103ed3e8db196b7d49e25cd0512707))
* **read:** apply diff-resend on all routes, not just client ([#182](https://github.com/FerroxLabs/wayland-core/issues/182)) ([da02c73](https://github.com/FerroxLabs/wayland-core/commit/da02c738ee824f755949dc0e7503968549c713de))
* **read:** reject UNC + non-regular paths in validate_user_path ([#644](https://github.com/FerroxLabs/wayland-core/issues/644)) ([#222](https://github.com/FerroxLabs/wayland-core/issues/222)) ([06d16aa](https://github.com/FerroxLabs/wayland-core/commit/06d16aa1fd0b35fb7f7e9bd042710893ae4bfbd3))
* repair 3 flaky main tests, fence release-binary smoke ([#192](https://github.com/FerroxLabs/wayland-core/issues/192)) ([076c0e1](https://github.com/FerroxLabs/wayland-core/commit/076c0e1de62ca7f7b1af9c56a5b903057552dbfc))
* **security:** confine untrusted project [@includes](https://github.com/includes) to the repo root ([#204](https://github.com/FerroxLabs/wayland-core/issues/204)) ([3e495ff](https://github.com/FerroxLabs/wayland-core/commit/3e495ffd8a4e362d91a3b8e68ca660430562bb94))
* **security:** neutralize untrusted project system_prompt ([#205](https://github.com/FerroxLabs/wayland-core/issues/205)) ([1b48357](https://github.com/FerroxLabs/wayland-core/commit/1b48357a4a1e217bb1a48b40f6810af025dc8b89))
* **sentinel:** per-pid crash-sentinel scoping (start-time ownership, delete-is-claim) ([#185](https://github.com/FerroxLabs/wayland-core/issues/185)) ([73f8104](https://github.com/FerroxLabs/wayland-core/commit/73f81047d8578163fc1fbbe42c62ba0e79f01bc0))
* **tools:** deny project secrets for Full-posture channel sessions ([#229](https://github.com/FerroxLabs/wayland-core/issues/229)) ([fdf68b1](https://github.com/FerroxLabs/wayland-core/commit/fdf68b19611a006e38d985691b6c174de3e42d6e))
* **tools:** drop Grep/Glob from Full channel-remote posture ([#235](https://github.com/FerroxLabs/wayland-core/issues/235)) ([17517e3](https://github.com/FerroxLabs/wayland-core/commit/17517e37fff3e1d2938ba238ecc3094f86193b3d))
* **tui:** anchor stale test clock; avoid Windows Instant underflow ([#225](https://github.com/FerroxLabs/wayland-core/issues/225)) ([6080a0f](https://github.com/FerroxLabs/wayland-core/commit/6080a0f3531c13d035171aaaa0f025d346851159))


### Documentation

* **design:** Anvil native gated-forge engine spec v1 ([#227](https://github.com/FerroxLabs/wayland-core/issues/227)) ([af893eb](https://github.com/FerroxLabs/wayland-core/commit/af893eb1a56e5902c5b1038bd73489317da07669))

## [0.12.23](https://github.com/FerroxLabs/wayland-core/compare/v0.12.22...v0.12.23) (2026-07-05)

A capabilities-and-honesty release. The engine now reasons in images across
every provider, extracts text from office documents, and is candid about what it
can and cannot do — while three fixes make failures loud, close a web-policy
bypass, and keep network access bounded to genuinely-local sessions.


### Highlights

* **Images as first-class content across all providers** (#648). A new
  `ContentBlock::Image` is encoded consistently for every OpenAI-compatible
  model, gated by a real `supports_vision` capability check so vision-blind
  models fail clearly instead of silently dropping the image.
* **`vision_analyze` accepts local image files** (#637), not just URLs — point it
  at a path on disk and it reads the bytes directly.
* **Office-document extraction** (#650, Phase 1). A new `doc_extract` tool pulls
  text out of office documents, with an explicit truncation contract so callers
  know when output was cut.
* **Honest capability availability** (#660). A boot-time advisory and
  channel-media notices tell you up front what the running configuration can
  actually do, instead of discovering a gap mid-task.


### Reliability

* **Consecutive tool failures are counted globally, not per-tool** (#160), so a
  model alternating between two failing tools still trips the runaway-loop cap.
* **Oversized tool outputs are shed before a context-overflow abort** (#636), and
  **silently-undersized context windows are corrected with a drift guard**
  (#165) so long sessions degrade gracefully instead of dying.
* **Conversation-heavy overflow degrades cleanly** at the second compaction rung
  (#646).
* **Per-assistant scoping for config MCP servers** (#111) keeps one assistant's
  MCP configuration from leaking into another's.


### Security & correctness

* **Website policy fails CLOSED** (#662). When a website-access policy cannot be
  evaluated (present but malformed config), access is denied rather than allowed
  — closing a bypass at the single chokepoint every caller funnels through.
* **`network=Inherit` is gated to genuinely-local sessions only** (#657). A
  channel- or Full-posture session no longer inherits ambient network access;
  only a session with no channel tool-posture stays local-inherit.
* **Tools fail loud instead of empty-success** (#661). Swallowed failures that
  previously returned an empty success now surface as real errors.
* **Silent operator and feature toggles are logged** (#664) across memory, tools,
  and browser, so state changes are auditable.

## [0.12.22](https://github.com/FerroxLabs/wayland-core/compare/v0.12.21...v0.12.22) (2026-07-04)

A reliability release focused on runaway-loop resilience and honest tool-error
signals. Agents that would previously burn a whole turn retrying a failing tool
now stop cleanly and let the host offer a "Continue"; MCP tool failures are
finally visible to the agent instead of masquerading as success; and two
hardening fixes close an out-of-memory vector and a duplicate-connection bug.


### Highlights

* **Retry-cap for stuck tool loops** (#475). A model that keeps calling a tool
  that keeps failing — e.g. an MCP call retried with a new wrong argument each
  time — no longer burns the turn's budget mid-thought. A per-run failure cap
  stops the run with clear guidance and, paired with the finish-reason work
  below, lets the host offer **Continue** to resume with fresh headroom. Tunable
  via `WAYLAND_MAX_CONSECUTIVE_TOOL_FAILURES` (the shell tool is deliberately
  exempt so a normal build/test burst is never mistaken for a stuck loop).
* **MCP tool failures are now honest** (#475). Every MCP tool-level failure —
  argument-validation errors, API errors — used to look like *success* to the
  agent, the error badge, and the model's own error signal, because the MCP
  `isError` flag was dropped on the way in. It now propagates end-to-end, so
  failures are visible (and the retry-cap can see them). The error text still
  reaches the model so it can read it and recover.
* **"Out of turns" now offers Continue, not "use a bigger model"** (#457). When
  a run hits its per-turn cap, the engine emits a distinct `max_turns`
  finish-reason (mapped to the ACP `max_turn_requests` stop) so hosts render a
  resume affordance instead of a model-failure message.


### Hardening

* **Bounded the OpenAI chat-path tool-call accumulator** (#136). A runaway or
  hostile streaming response can no longer allocate unbounded tool-call slots;
  an out-of-range streamed index now fails the stream closed.
* **`/mcp add` is idempotent** (#135). Re-adding an already-connected MCP server
  no longer spawns a duplicate connection — or, for stdio servers, a duplicate
  child process.


### Tests & docs

* **WebFetch extraction-timeout coverage** (#110). The readability
  extraction-timeout → raw-body fallback is now tested with an injected slow
  extractor, and the orphaned-thread behavior is documented honestly.

## [0.12.21](https://github.com/FerroxLabs/wayland-core/compare/v0.12.20...v0.12.21) (2026-07-03)

A security and reliability release. It closes GHSA-8r7g end-to-end on the ACP
transport — the secret approval `resume_token` is now carried to the host and
required on the wire, so a model can no longer self-approve a bridge-backed gate
— and it fixes a Windows regression that had left users unable to run any
command at all.


### Security

* **acp:** carry + accept the secret `resume_token` end-to-end on the ACP transport (GHSA-8r7g M2) — the projection now surfaces the server-minted `apr-` secret to the host, and the `/resolve` endpoint routes it to the approval bridge (secret-preferred, falling back to the manager's `call_id` path). Bridge-backed gates (Crucible council / egress consent) raised during an ACP turn are now resolvable instead of hanging to their TTL, and the model-self-approval class is closed for them ([#152](https://github.com/FerroxLabs/wayland-core/issues/152)) ([ce63ca6](https://github.com/FerroxLabs/wayland-core/commit/ce63ca64))
* **acp:** stamp the bridge secret `resume_token` on the ACP relay (GHSA-8r7g M1) — bridge-backed gate frames on the ACP relay now carry the secret token at parity with the stdin/TUI transports ([#147](https://github.com/FerroxLabs/wayland-core/issues/147)) ([98c387b](https://github.com/FerroxLabs/wayland-core/commit/98c387b4))


### Bug Fixes

* **sandbox:** drain the Windows AppContainer stdout/stderr pipes concurrently with the child — a P1 that had left Windows users unable to run **any** command: output past the ~4 KB pipe buffer blocked the child, which timed the wait out and returned blank or truncated results. Reader threads now drain both pipes while the child runs; live-verified on the Windows CI leg ([#151](https://github.com/FerroxLabs/wayland-core/issues/151)) ([1619632](https://github.com/FerroxLabs/wayland-core/commit/1619632d))
* **sandbox:** skip missing bwrap bind sources instead of fail-spawning — a manifest-declared mount whose source is absent on a fresh HOME no longer aborts the sandbox spawn (`--bind-try` / `--ro-bind-try`); fixes a model-agnostic empty-bash hang ([#148](https://github.com/FerroxLabs/wayland-core/issues/148)) ([d434dd5](https://github.com/FerroxLabs/wayland-core/commit/d434dd51))
* **cli:** a mid-turn Stop must not strand the json-stream session — Stop now cancels the in-flight turn but keeps the session alive; only EOF and `/exit` end it, restoring the json-stream protocol §2.2 contract ([#150](https://github.com/FerroxLabs/wayland-core/issues/150)) ([d2dd423](https://github.com/FerroxLabs/wayland-core/commit/d2dd4238))
* **cli:** honor the config `[default] approval_mode` in json-stream mode — json-stream sessions now apply the configured approval posture, not only `--force` ([#149](https://github.com/FerroxLabs/wayland-core/issues/149)) ([b042e32](https://github.com/FerroxLabs/wayland-core/commit/b042e32e))
* **cli:** emit the json-stream `ready` frame before config MCP servers connect — the host sees `ready` immediately; configured MCP servers integrate in the background and settle at the next command boundary ([#146](https://github.com/FerroxLabs/wayland-core/issues/146)) ([56953b6](https://github.com/FerroxLabs/wayland-core/commit/56953b6e))


### Features

* **channels:** per-conversation autonomous-send rate cap — a runaway ping-pong backstop that caps autonomous auto-replies per conversation (default 30 / 10 min) so two agents wired to the same channel can't loop forever burning cost and quota. Human and operator sends bypass it entirely ([#154](https://github.com/FerroxLabs/wayland-core/issues/154)) ([876f4e5](https://github.com/FerroxLabs/wayland-core/commit/876f4e52))

## [0.12.20](https://github.com/FerroxLabs/wayland-core/compare/v0.12.19...v0.12.20) (2026-07-02)


### Features

* **agent:** host-delegated send_message hook (host-send-transport) with a hard approval gate — `WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1` routes sends through a call_id-correlated json-stream round-trip; approval always fronts the request (Exec category, never auto-approved, Always-scope downgrades to Once) ([#141](https://github.com/FerroxLabs/wayland-core/issues/141)) ([#144](https://github.com/FerroxLabs/wayland-core/issues/144)) ([5bb5899](https://github.com/FerroxLabs/wayland-core/commit/5bb5899e))


### Bug Fixes

* **agent:** honor omitted `--max-tokens` with per-model output sizing — known models size to their real ceiling; unknown models on omit-safe providers (gemini/openrouter/flux) omit the wire field so the provider's natural ceiling applies; anthropic/generic keep the sized floor; explicit caps and per-spawn sub-agent caps always bind ([#112](https://github.com/FerroxLabs/wayland-core/issues/112)) ([#138](https://github.com/FerroxLabs/wayland-core/issues/138)) ([3ecbc2c](https://github.com/FerroxLabs/wayland-core/commit/3ecbc2c4))
* **providers:** call_id stability for parallel/builtin/skill tool calls on the OpenAI-responses (ChatGPT/Codex) adapter — never-empty ids, no cross-wiring of interleaved items, bounded accumulators; fixes the desktop stuck-spinner class ([#133](https://github.com/FerroxLabs/wayland-core/issues/133)) ([#137](https://github.com/FerroxLabs/wayland-core/issues/137)) ([90e0fb2](https://github.com/FerroxLabs/wayland-core/commit/90e0fb2e))
* **sandbox:** bound the Windows AppContainer availability probe (15s wall-clock guard) with a short-TTL negative cache — commands no longer hang ~120s per invocation when the probe stalls; fail-closed posture unchanged ([#125](https://github.com/FerroxLabs/wayland-core/issues/125)) ([#127](https://github.com/FerroxLabs/wayland-core/issues/127)) ([263793b](https://github.com/FerroxLabs/wayland-core/commit/263793b9))
* **npm:** staleness self-heal in the npx launcher — warns with the exact-version cache-busting command when the spec-keyed npx cache serves an old engine; docs pinned to `@latest` ([#126](https://github.com/FerroxLabs/wayland-core/issues/126)) ([#134](https://github.com/FerroxLabs/wayland-core/issues/134)) ([3ebbc72](https://github.com/FerroxLabs/wayland-core/commit/3ebbc72c))


### Tests & Hardening

* **providers:** tool-name codec round-trip regression suite incl. the direct-DeepSeek delegation pin ([#139](https://github.com/FerroxLabs/wayland-core/issues/139)) ([#140](https://github.com/FerroxLabs/wayland-core/issues/140)) ([abe516e](https://github.com/FerroxLabs/wayland-core/commit/abe516e2))
* **security:** quick-xml RUSTSEC-2026-0194/0195 dispositioned (unreachable — embedded-dump-only syntect path), time-boxed with tracking ([#142](https://github.com/FerroxLabs/wayland-core/issues/142)) ([#143](https://github.com/FerroxLabs/wayland-core/issues/143)) ([4e74ddb](https://github.com/FerroxLabs/wayland-core/commit/4e74ddb5))

## [0.12.19](https://github.com/FerroxLabs/wayland-core/compare/v0.12.18...v0.12.19) (2026-07-01)

A security-hardening release. Wayland Core tightens every seam where an untrusted
input — a checked-in project config, a wire peer, or a model-known identifier —
could quietly widen its own privileges. All fixes below are part of the
coordinated **GHSA-8r7g** approval/posture-hardening advisory and are covered by
new regression tests.

### Security Hardening (GHSA-8r7g)

* **Project configs can only tighten, never loosen.** A `.wayland-core.toml` that
  travels with a cloned repo is untrusted. It can no longer raise the approval
  posture — `approval_mode`, `auto_approve`, `allow_no_sandbox`, and the
  approval-skip `allow_list` are all clamped tighten-only against your global
  config, so opening a repo can never silently reduce approval friction or grant
  a tool blanket auto-approval ([#128](https://github.com/FerroxLabs/wayland-core/issues/128)).
* **Project-defined hooks are default-denied.** A `[[hooks.*]]` `command` runs as
  a child process, so a project hook is arbitrary code execution from repo
  content. Project hooks are now dropped unless the operator opts in with
  `[hooks] trust_project_hooks = true` in their **global** config; a project
  cannot authorize its own hooks, and suppressed hooks are logged (not silent).
* **Auto-approving modes require a local opt-in over the wire.** Both `force` and
  `auto_edit` auto-approve tools (`auto_edit` covers file writes — a git hook or
  `authorized_keys` write is write-to-RCE). Neither can now be set by an
  un-opted-in wire peer; only `default` is accepted unless the operator launched
  with `--force` / `WAYLAND_ALLOW_WIRE_FORCE=1`.
* **Unforgeable approval resume tokens.** The in-process approval bridge now keys
  every pending approval by an opaque secret, indexing the public correlation id
  (a model-known `call_id`) separately. A wire/host peer must present the secret
  to resolve an approval — echoing a known `call_id` no longer self-approves —
  while local TUI keypresses resolve by correlation as before. (Also closes a
  latent gap where a bridge-backed approval — a Crucible council or an egress
  consent — parked mid-turn could hang on a JSON-stream host.)
* **Project skill hooks are default-denied too.** A project- or legacy-sourced
  skill's `SKILL.md` frontmatter hooks run as child processes, so — like project
  config hooks — they are now dropped unless the operator opted in via the global
  `[hooks] trust_project_hooks`. A cloned repo's skill can no longer execute a
  hook on first tool use.

### Bug Fixes

* **providers:** sanitize Cohere MCP tool names through the shared codec so
  MCP-heavy profiles no longer 400 on Cohere, matching the cross-provider
  handling introduced for the other providers
  ([#129](https://github.com/FerroxLabs/wayland-core/issues/129)) ([#131](https://github.com/FerroxLabs/wayland-core/issues/131)).
* **providers:** sanitize MCP tool names across all provider paths so MCP-heavy
  profiles stop hitting name-shape 400s ([#130](https://github.com/FerroxLabs/wayland-core/issues/130)).

## [0.12.18](https://github.com/FerroxLabs/wayland-core/compare/v0.12.17...v0.12.18) (2026-07-01)


### Bug Fixes

* **providers:** never send a role:"tool" message without a matching assistant tool_calls id — make truncation tool-pair aware and strip orphaned tool results/calls in both directions so native DeepSeek no longer 400s on long agentic conversations ([#123](https://github.com/FerroxLabs/wayland-core/issues/123)) ([#124](https://github.com/FerroxLabs/wayland-core/issues/124)) ([bf82b05](https://github.com/FerroxLabs/wayland-core/commit/bf82b050))
* **providers:** keep an assistant message valid after its last tool_call is stripped (stamp empty content) so native DeepSeek no longer 400s with "content or tool_calls must be set" — found via live verification against api.deepseek.com ([#123](https://github.com/FerroxLabs/wayland-core/issues/123)) ([#124](https://github.com/FerroxLabs/wayland-core/issues/124)) ([bf82b05](https://github.com/FerroxLabs/wayland-core/commit/bf82b050))

## [0.12.17](https://github.com/FerroxLabs/wayland-core/compare/v0.12.16...v0.12.17) (2026-06-30)


### Bug Fixes

* **agent:** resolve send_message channel by platform family so named instance channels (e.g. email-imap) receive sends ([#116](https://github.com/FerroxLabs/wayland-core/issues/116)) ([#117](https://github.com/FerroxLabs/wayland-core/issues/117)) ([82b590c](https://github.com/FerroxLabs/wayland-core/commit/82b590c3))
* **agent:** cap project-context (AGENTS.md / @-includes) injection to bound the cached system prefix ([#115](https://github.com/FerroxLabs/wayland-core/issues/115)) ([#118](https://github.com/FerroxLabs/wayland-core/issues/118)) ([9cdf420](https://github.com/FerroxLabs/wayland-core/commit/9cdf420e))
* **providers:** strip internal extra_content from outbound tool_calls so long-context replay to strict providers no longer 400s ([#120](https://github.com/FerroxLabs/wayland-core/issues/120)) ([#121](https://github.com/FerroxLabs/wayland-core/issues/121)) ([525a90f](https://github.com/FerroxLabs/wayland-core/commit/525a90f2))


### Dependencies

* clear release security gate — wasmtime 36.0.11 → 36.0.12 (RUSTSEC-2026-0188), anyhow 1.0.102 → 1.0.103 (RUSTSEC-2026-0190), ttf-parser OSV disposition (RUSTSEC-2026-0192) ([#119](https://github.com/FerroxLabs/wayland-core/issues/119)) ([db3797f](https://github.com/FerroxLabs/wayland-core/commit/db3797fd))

## [0.12.16](https://github.com/FerroxLabs/wayland-core/compare/v0.12.15...v0.12.16) (2026-06-29)


### Bug Fixes

* **bash:** fall back to cmd when PowerShell shell is selected under AppContainer ([#105](https://github.com/FerroxLabs/wayland-core/issues/105)) ([d698c66](https://github.com/FerroxLabs/wayland-core/commit/d698c663f0f361912ed25f532a83e519305c246a))
* **engine:** never let the reasoning budget starve the visible answer ([#426](https://github.com/FerroxLabs/wayland-core/issues/426)) ([#107](https://github.com/FerroxLabs/wayland-core/issues/107)) ([60f8e7d](https://github.com/FerroxLabs/wayland-core/commit/60f8e7d649a4a2fa684c4228b620a9ea8d0491fd))
* **providers:** replay reasoning_content for strict reasoners routed via a router ([#417](https://github.com/FerroxLabs/wayland-core/issues/417)) ([#108](https://github.com/FerroxLabs/wayland-core/issues/108)) ([fac4bde](https://github.com/FerroxLabs/wayland-core/commit/fac4bde7eecc2b3c31ec7c20927034432eea4bfa))
* **web:** bound readability extraction + reset breakers per turn + telemetry schema ([#403](https://github.com/FerroxLabs/wayland-core/issues/403)) ([#106](https://github.com/FerroxLabs/wayland-core/issues/106)) ([43c7aac](https://github.com/FerroxLabs/wayland-core/commit/43c7aac819e649c72383edd58be446d94856ace7))

## [0.12.15](https://github.com/FerroxLabs/wayland-core/compare/v0.12.14...v0.12.15) (2026-06-28)


### Bug Fixes

* **providers:** keyless self-hosted endpoints (no more "OpenAI API key is required" on local Ollama) ([#102](https://github.com/FerroxLabs/wayland-core/issues/102)) ([28d5eac](https://github.com/FerroxLabs/wayland-core/commit/28d5eac64851b9e404bd371f59768ee41890d9e9))

## [0.12.14](https://github.com/FerroxLabs/wayland-core/compare/v0.12.13...v0.12.14) (2026-06-28)

A focused Windows reliability release: it makes the sandboxed shell tool work end-to-end on Windows, fixing two AppContainer defects that left tool-use broken in the field.

### Highlights

- **Windows shell tools no longer hard-fail on machines without dev caches.** The AppContainer filesystem allowlist always includes optional developer caches (`~/.cache`, `~/.cargo`, `~/.npm`, `~/.rustup`). On any machine that doesn't have them — i.e. virtually every non-developer Windows box — applying the DACL grant aborted the *entire* command with `GetNamedSecurityInfoW … 0x2`, so every sandboxed shell command failed before it ran. Absent allowlist paths are now skipped, the grant succeeds, and commands execute normally. This is why the earlier AppContainer subprocess fixes ([#321](https://github.com/FerroxLabs/wayland-core/issues/321)–[#324](https://github.com/FerroxLabs/wayland-core/issues/324)) didn't translate into working shells in the field.
- **Sandboxed commands can no longer hang past their timeout.** `cmd.exe` spawns a console host (`conhost.exe`) that can outlive the command and keep the captured stdout/stderr pipes open; the output drain then blocked waiting for an EOF that never arrived — observed as a 120-second "command timed out" with no output on disconnected RDP sessions. The backend now reaps the entire job tree before draining, so output always flushes and the call returns a bounded result (or a clean, prompt timeout) instead of hanging. ([#100](https://github.com/FerroxLabs/wayland-core/issues/100))

## [0.12.13](https://github.com/FerroxLabs/wayland-core/compare/v0.12.12...v0.12.13) (2026-06-27)

A reliability-focused release: a new **capability-first tools gate** so models that can't do function calling degrade gracefully instead of failing the turn, a major Windows sandbox fix, and a round of audited provider- and config-layer hardening.

### Highlights

- **Tool-incapable models just work now — across local and cloud backends.** Point Wayland Core at a model that doesn't support function calling and the turn no longer dies on a raw provider error. Ollama models are detected up front via `/api/show` and have their `tools` array dropped before the request is even sent. Any backend that rejects tools with a `400` — llama.cpp started without `--jinja` (`tools param requires --jinja flag`), or an Ollama model that 400s with `does not support tools` — is caught, retried without tools, and **remembered**, so every later turn for that model skips tools pre-emptively. Tool-incapable Bedrock models (DeepSeek-R1 reasoning, Stability image, Titan/Cohere embedding) are name-gated the same way. Tool-*capable* models are unaffected — they keep their tools and call them exactly as before. ([#389](https://github.com/FerroxLabs/wayland-core/issues/389))
- **The Windows sandbox runs real subprocesses again.** The AppContainer backend no longer caps active processes too aggressively (`ActiveProcessLimit` raised to 512), resolves the launch shell correctly, and emits clearer diagnostics when a shell can't be found — so multi-step tool use works under the sandbox on Windows. ([#321](https://github.com/FerroxLabs/wayland-core/issues/321), [#322](https://github.com/FerroxLabs/wayland-core/issues/322), [#323](https://github.com/FerroxLabs/wayland-core/issues/323), [#324](https://github.com/FerroxLabs/wayland-core/issues/324))

### Provider reliability

- **Anthropic errors are classified correctly.** Non-credit Anthropic API errors are no longer misread as out-of-credit / billing failures, so genuine transient errors surface instead of a misleading "purchase credits" signal. ([#329](https://github.com/FerroxLabs/wayland-core/issues/329))
- **Flux reasoning summaries render as thinking.** A FluxRouter `reasoning_summary` is now decoded into a per-turn thinking subject, so reasoning summaries appear as proper thinking content. ([#318](https://github.com/FerroxLabs/wayland-core/issues/318))

### Configuration & hygiene

- **Config surface tightened.** `env_passthrough` is now wired through, unknown configuration keys produce a warning (via `serde_ignored`) instead of being silently dropped, and the sandbox configuration surface is exposed as a toggle. ([#325](https://github.com/FerroxLabs/wayland-core/issues/325), [#326](https://github.com/FerroxLabs/wayland-core/issues/326), [#327](https://github.com/FerroxLabs/wayland-core/issues/327))

## [0.12.12](https://github.com/FerroxLabs/wayland-core/compare/v0.12.11...v0.12.12) (2026-06-27)

### Crucible reliability & cost accuracy

This release hardens the Crucible (Mixture-of-Providers) council and the pricing engine behind it — every fix here was found by putting Crucible through a live, cross-vendor proof run and watching where it strained.

- **Bring-your-own pricing catalogs now load.** A custom `WAYLAND_PRICING_PATH` catalog parses reliably, so you can price any model the bundled catalog doesn't yet cover — and Crucible can certify a real spend ceiling against it.
- **Accurate Gemini pricing.** Gemini's live API slugs (e.g. `gemini-2.5-flash`) now resolve to the catalog correctly, so Gemini members are priced — and counted — when Crucible assembles a cost-diverse council.
- **Broader Opus support in councils.** Anthropic's Opus 4.x models, which decline an explicit sampling temperature, are now handled cleanly both as proposers and as the fusing judge.

Backed by new regression tests across `wcore-pricing` and `wcore-providers`.

## [0.12.11](https://github.com/FerroxLabs/wayland-core/compare/v0.12.10...v0.12.11) (2026-06-27)

This release is headlined by **Crucible**, our cross-provider Mixture-of-Providers council — wayland-core's answer to single-model ceilings — folded together with two audited reliability and security fixes.

### ✨ Headliner — Crucible (Mixture-of-Providers)

* **crucible:** a cross-provider council you run with `wayland-core crucible "<task>"`. N proposers, **each pinned to a different LLM provider**, work the task in parallel; a fenced, read-only **aggregator** fuses their answers into one. Three ways to run it: `--auto` gates convening behind a cheap difficulty classifier (trivial tasks get a single direct call, high-stakes tasks convene the full council); `--advisor` injects the fused synthesis into the normal trusted agent loop as private guidance (the agent then reasons, acts, and uses tools on it); `--terminal` prints the fused answer and stops. Includes per-tier proposer/aggregator temperatures, provenance-fenced injection containment, per-proposer **and** global soft deadlines with quorum, and `[crucible]` budget/daily-cap guards. Tri-model cross-audited; 151 dedicated tests. ([#91](https://github.com/FerroxLabs/wayland-core/pull/91))

### Enhancements

* **tools:** `image_generate` and `text_to_speech` now follow your active provider instead of assuming a single hardcoded host. FluxRouter and native OpenAI sessions route to the correct endpoint with the correct key (with proper `/v1` API-root resolution), gracefully fall back to FAL / Gemini Imagen / Hugging Face FLUX via their env keys, and **fail closed** on a base URL carrying embedded credentials. ([#310](https://github.com/FerroxLabs/wayland/issues/310))

### Security & Hardening

* **mcp:** MCP tool curation is now driven purely by **BM25 relevance + recency**. Removed a name-based "rescue" boost that a third-party MCP server could exploit by naming a tool like a built-in to jump the curation budget — closing a budget-hijack vector with no impact on built-in tools (which are never curated). ([#89](https://github.com/FerroxLabs/wayland/issues/89))

### Validation

* Full cross-platform gate green — **9,411 tests** across Linux, macOS, and Windows.

## [0.12.10](https://github.com/FerroxLabs/wayland-core/compare/v0.12.9...v0.12.10) (2026-06-27)


### Features

* **mcp:** provider-aware hard cap on total tool count + real MCP server provenance + BM25 relevance curation — caps the outbound tool array to the model's limit (OpenAI 128), fixing API-400 overflow with large MCP servers (Google Workspace, etc.); fixes uniquely-named MCP tools being misclassified as built-ins ([#86](https://github.com/FerroxLabs/wayland-core/issues/86), [#344](https://github.com/FerroxLabs/wayland-core/issues/344)/[#359](https://github.com/FerroxLabs/wayland-core/issues/359)) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87))


### Bug Fixes

* **deps:** bump pdf-extract 0.12 → lopdf 0.42 ([RUSTSEC-2026-0187](https://rustsec.org/advisories/RUSTSEC-2026-0187)) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87))
* **web-fetch:** wall-clock timeout message now contains "timed out" (de-flake) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87))

## [0.12.9](https://github.com/FerroxLabs/wayland-core/compare/v0.12.8...v0.12.9) (2026-06-25)


### Bug Fixes

* OpenAI tool-name sanitization ([#297](https://github.com/FerroxLabs/wayland-core/issues/297)) + WSL canonicalize off-reactor ([#287](https://github.com/FerroxLabs/wayland-core/issues/287)) ([#84](https://github.com/FerroxLabs/wayland-core/issues/84)) ([af69bdc](https://github.com/FerroxLabs/wayland-core/commit/af69bdc046bef94671426a20a8a1fb7327c91d30))

## [0.12.8](https://github.com/FerroxLabs/wayland-core/compare/v0.12.7...v0.12.8) (2026-06-24)


### Features

* **providers:** add Sakana AI (Fugu) — OpenAI-compatible endpoint ([#82](https://github.com/FerroxLabs/wayland-core/issues/82)) ([a531f22](https://github.com/FerroxLabs/wayland-core/commit/a531f220d9ffbc089815b9dfb78478ff6affa4bd))

## [0.12.7](https://github.com/FerroxLabs/wayland-core/compare/v0.12.6...v0.12.7) (2026-06-23)


### Features

* **#255:** active-window kernel — context % vs the post-swap active model ([#74](https://github.com/FerroxLabs/wayland-core/issues/74)) ([7d22c84](https://github.com/FerroxLabs/wayland-core/commit/7d22c847718e48871bde90d666c906de350aecb8))
* **#279:** JSON-stream observability — active-window %, agent-run correlation, structured traces ([#76](https://github.com/FerroxLabs/wayland-core/issues/76)) ([3b9b070](https://github.com/FerroxLabs/wayland-core/commit/3b9b07006f399af3ccd9689166d028d94f2de003))
* **#280:** smart auto-compaction at active-window threshold (default-off, Flux-aware, memory handoff) ([#78](https://github.com/FerroxLabs/wayland-core/issues/78)) ([508d9e8](https://github.com/FerroxLabs/wayland-core/commit/508d9e8e790771f23f82b8577edecfd511624096))
* **#282:** Flux context-routing contract — client side V1 ([#77](https://github.com/FerroxLabs/wayland-core/issues/77)) ([508af81](https://github.com/FerroxLabs/wayland-core/commit/508af81c533b36e0cdedc0e48f55e6f695c70e1d))
* isolated profiles — CLI-isolation slice (Phase 0 + 1 + 3A + 2) ([#70](https://github.com/FerroxLabs/wayland-core/issues/70)) ([3177b17](https://github.com/FerroxLabs/wayland-core/commit/3177b1763d0334ba03057992d689904b9f810554))


### Bug Fixes

* **#282:** tolerate live Flux context-overflow shapes (found by live E2E) ([#79](https://github.com/FerroxLabs/wayland-core/issues/79)) ([c5aadd6](https://github.com/FerroxLabs/wayland-core/commit/c5aadd636505fb008f5dfa735ff9b09d2b0fe18c))
* **#285:** never emit orphaned tool_result during compaction (DeepSeek 400) ([#75](https://github.com/FerroxLabs/wayland-core/issues/75)) ([5f3aaf7](https://github.com/FerroxLabs/wayland-core/commit/5f3aaf78d01d9bab3fbf80766e97761f024eb4df))
* **#293:** authenticate openai-chatgpt from ~/.codex/auth.json ([#80](https://github.com/FerroxLabs/wayland-core/issues/80)) ([7f0c7cc](https://github.com/FerroxLabs/wayland-core/commit/7f0c7cc1559526f5a5814fd72a8a099500218699))
* OpenAI image default (gpt-image-1) + DeepSeek v4-flash 1M context ([#265](https://github.com/FerroxLabs/wayland-core/issues/265), [#255](https://github.com/FerroxLabs/wayland-core/issues/255)) ([#69](https://github.com/FerroxLabs/wayland-core/issues/69)) ([30dad57](https://github.com/FerroxLabs/wayland-core/commit/30dad572cb15b2ff3cdb0d7f2b936525d7e5ac06))
* **windows:** 4 Windows-only failures ([#257](https://github.com/FerroxLabs/wayland-core/issues/257) CRLF edit, [#262](https://github.com/FerroxLabs/wayland-core/issues/262)/[#263](https://github.com/FerroxLabs/wayland-core/issues/263) MCP stdio quoting, [#267](https://github.com/FerroxLabs/wayland-core/issues/267) sandbox \\?\ path) ([#72](https://github.com/FerroxLabs/wayland-core/issues/72)) ([d7ccbef](https://github.com/FerroxLabs/wayland-core/commit/d7ccbef78194fbbb7ad5ed7e87c7f0afb5370f0f))

## [0.12.6](https://github.com/FerroxLabs/wayland-core/compare/v0.12.5...v0.12.6) (2026-06-22)


### Features

* ChatGPT-sub model filtering ([#158](https://github.com/FerroxLabs/wayland-core/issues/158)) + MiniMax cost catalog ([#240](https://github.com/FerroxLabs/wayland-core/issues/240)) ([#68](https://github.com/FerroxLabs/wayland-core/issues/68)) ([f807397](https://github.com/FerroxLabs/wayland-core/commit/f807397dab29b9eea1fe18a9ef0f80e9ead3edfd))
* FluxRouter capabilities (image/fetch/web_search) + per-model max_tokens + reliability fixes ([#66](https://github.com/FerroxLabs/wayland-core/issues/66)) ([aefdd39](https://github.com/FerroxLabs/wayland-core/commit/aefdd3993c47c0a0ba6e6c7f16fbaf917cc325cd))


### Performance Improvements

* **token-spend:** wire routing tier, cheap+accurate compaction, bound retries, cache hygiene ([#65](https://github.com/FerroxLabs/wayland-core/issues/65)) ([2c70b7b](https://github.com/FerroxLabs/wayland-core/commit/2c70b7b828eb5f4defb4f60f29492d9c3fedf129))

## [0.12.5](https://github.com/FerroxLabs/wayland-core/compare/v0.12.4...v0.12.5) (2026-06-21)


### Features

* **sandbox:** WorkspacePolicy + OS secret-read-deny + Landlock Option A ([#59](https://github.com/FerroxLabs/wayland-core/issues/59)) ([dfa5aa2](https://github.com/FerroxLabs/wayland-core/commit/dfa5aa29c9d4f2a7cdf363f701339ed5147e37ad))


### Bug Fixes

* **#200:** unblock native Gemini egress + stop silent finish_reason=error turns ([#60](https://github.com/FerroxLabs/wayland-core/issues/60)) ([8d95578](https://github.com/FerroxLabs/wayland-core/commit/8d955782faf43d8c473606537337db0384ad0e9e))
* **agent,tools:** close two real Windows bugs (unbounded project-context walk + glob sandbox bypass) ([#64](https://github.com/FerroxLabs/wayland-core/issues/64)) ([fea2c52](https://github.com/FerroxLabs/wayland-core/commit/fea2c52f6069f1e32f1bfbcb7640818a7820b397))
* **cli:** surface a clear, Ollama-aware reason on init failure instead of bare exit 1 ([#186](https://github.com/FerroxLabs/wayland-core/issues/186)) ([#61](https://github.com/FerroxLabs/wayland-core/issues/61)) ([b37b3d1](https://github.com/FerroxLabs/wayland-core/commit/b37b3d12663fdf45b472933bf5eb12f0164fc8db))
* **shell:** accept .exe and absolute-path Windows shell selectors ([#197](https://github.com/FerroxLabs/wayland-core/issues/197)) ([#62](https://github.com/FerroxLabs/wayland-core/issues/62)) ([9b332e7](https://github.com/FerroxLabs/wayland-core/commit/9b332e7eedc9bf4ec9141dbbdceaff6b01a3873b))

## [0.12.4](https://github.com/FerroxLabs/wayland-core/compare/v0.12.3...v0.12.4) (2026-06-20)


### Bug Fixes

* **skills:** hide unreviewed auto-drafted skills from the model catalog ([#56](https://github.com/FerroxLabs/wayland-core/issues/56)) ([a2c0de4](https://github.com/FerroxLabs/wayland-core/commit/a2c0de415e8ce51ee8f0232b8590119276d6e152))
* **skills:** keep the hello test fixture out of the shipped catalog ([#55](https://github.com/FerroxLabs/wayland-core/issues/55)) ([35d334f](https://github.com/FerroxLabs/wayland-core/commit/35d334f7f10b7ca215fb1c674fbb7c64e654f507))

## [0.12.3](https://github.com/FerroxLabs/wayland-core/compare/v0.12.2...v0.12.3) (2026-06-19)


### Features

* **tools:** PowerShell shell for the Bash tool on Windows — selectable via the `WAYLAND_BASH_SHELL` env var or the `[tools] windows_shell` config key (`powershell`/`pwsh`); precedence env > config > default `cmd`, scoped to the Bash tool ([#45](https://github.com/FerroxLabs/wayland-core/issues/45)) ([130dc3d](https://github.com/FerroxLabs/wayland-core/commit/130dc3da1d4720ac407423125f058aacb6c2390d))


### Bug Fixes

* **egress:** allowlist NVIDIA NIM, Cerebras, MiniMax-failover & Qwen hosts ([#48](https://github.com/FerroxLabs/wayland-core/issues/48)) ([a68f2d9](https://github.com/FerroxLabs/wayland-core/commit/a68f2d917f8c950004a9d92ba57cce9d759cbe4d))
* **oauth:** stop advertising a non-existent `wayland auth login grok` command ([#47](https://github.com/FerroxLabs/wayland-core/issues/47)) ([42e16ec](https://github.com/FerroxLabs/wayland-core/commit/42e16ec5009883a1cff42478f2d347ac4fee7a13))
* **providers:** strip empty/missing tool_call_id before sending (DeepSeek 400 guard) ([#50](https://github.com/FerroxLabs/wayland-core/issues/50)) ([c97424d](https://github.com/FerroxLabs/wayland-core/commit/c97424d463f5e976c1e2863db65cebaf74b0a6a7))


### Documentation

* refresh across the board for 0.12.x ([#46](https://github.com/FerroxLabs/wayland-core/issues/46)) ([273c764](https://github.com/FerroxLabs/wayland-core/commit/273c764af7a936b2dc8c73beaf82a310df55b7a2))


### Miscellaneous Chores

* release 0.12.3 ([cd03533](https://github.com/FerroxLabs/wayland-core/commit/cd03533fb210d9cf7cb5727407bfbd211ff5a4b4))

## [0.12.2](https://github.com/FerroxLabs/wayland-core/compare/v0.12.1...v0.12.2) (2026-06-18)


### Bug Fixes

* **providers:** provider auth robustness — Grok OAuth, region failover, auth errors ([#42](https://github.com/FerroxLabs/wayland-core/issues/42)) ([4dfc566](https://github.com/FerroxLabs/wayland-core/commit/4dfc566af50b6a233f4543e837f84efa5ee8490a))


### Miscellaneous Chores

* release 0.12.2 ([0323931](https://github.com/FerroxLabs/wayland-core/commit/03239313f4c02ec36f615cf5bcae7bf3b0590435))

## [0.12.1](https://github.com/FerroxLabs/wayland-core/compare/v0.12.0...v0.12.1) (2026-06-18)

Stable release rolling up everything from the `0.12.1-rc.1` and `0.12.1-rc.2`
prereleases (full per-commit detail in the sections below).

### Highlights

* **Sign in with ChatGPT** — OpenAI Codex OAuth provider with rotating-refresh token manager, device-code login for headless/remote, and token import from the Codex CLI.
* **MiniMax provider** — via the Anthropic-compatible endpoint, visible in the provider/model pickers.
* **Forge zero-config MCP discovery** — one-command `/mcp connect` to a trusted loopback MCP server, scoped-token grant with `${cred:KEY}` headers (token never lands in `config.toml`), opt-in `allow_local`, and a selectable DISCOVERED row in `/doctor`.
* **Config cockpit** — paste-to-connect with live key fingerprinting + a validation ladder, an Essentials/Advanced settings surface, collection editors (tools/egress/failover), config-posture health and self-configure discovery in `/doctor`, a redacted `/effective` config preview, and channel-integration visibility.
* **Live model discovery** — Bedrock (`ListFoundationModels`), Gemini, and a connected-provider catalog refresh, backed by a per-provider 24h disk cache.
* **TUI** — arrow-key cross-provider `/model` and `/provider` pickers, the command palette on `/` from any surface, connection-aware provider listing.
* **Security & stability** — a 42-defect deep-sweep remediation: closed a Forge-MCP token-exfil SSRF, a Glob sandbox bypass, unbounded reads across MCP/Matrix/ACP, a provider key-pool poison DoS, skill-arg shell injection, and MCP header secret leaks; credentials now default to keyring with plaintext fallback (F16).
* **Core fixes** — Windows MCP stdio launch (#164) and the Anthropic unrecoverable-conversation `thinking.signature` 400 (#161); Flux Router reachable out of the box under the egress guard.

### Build System

* **release:** promote 0.12.1 stable ([d50bfbb](https://github.com/FerroxLabs/wayland-core/commit/d50bfbb1f19d173d4fb56350d8ae633d583e7686))

## [0.12.1-rc.2](https://github.com/FerroxLabs/wayland-core/compare/v0.12.1-rc.1...v0.12.1-rc.2) (2026-06-18)


### Features

* **providers:** add MiniMax provider via Anthropic-compatible endpoint ([703ba14](https://github.com/FerroxLabs/wayland-core/commit/703ba14ce25f5b23a19a06cea00aebdb16631bc4))


### Bug Fixes

* **audit:** 19 low/medium defects — browser, sandbox, channels, tools, TUI ([8c589ad](https://github.com/FerroxLabs/wayland-core/commit/8c589ad36be0e4e8605ca1e49c770a52ce6f3385))
* **audit:** 7 high-severity defects — sandbox, provider protocol, unbounded reads ([8273b2a](https://github.com/FerroxLabs/wayland-core/commit/8273b2ac1e56937e816101c45415954a6d4ea6b6))
* **audit:** provider resilience + egress/secret hygiene (8 fixes) ([0e893d9](https://github.com/FerroxLabs/wayland-core/commit/0e893d99f38b623a4deaa65ea27d3c51c424c8eb))
* **config:** default credentials to keyring with plaintext fallback (F16) ([6c57160](https://github.com/FerroxLabs/wayland-core/commit/6c5716080da4429f32a0ccfc9acd0399cfe6bd3f))
* **core:** Windows MCP stdio launch ([#164](https://github.com/FerroxLabs/wayland-core/issues/164)) + Anthropic unrecoverable-conversation ([#161](https://github.com/FerroxLabs/wayland-core/issues/161)) ([38b85e6](https://github.com/FerroxLabs/wayland-core/commit/38b85e6fb6895100e24218366586b08da6dd62d4))
* **egress:** allowlist Flux Router out of the box + accept full-host entries ([1fa6407](https://github.com/FerroxLabs/wayland-core/commit/1fa6407e907227e7c09b7431e968dbd3920e95d0))
* **forge-mcp:** close token-exfil SSRF + 4 reliability defects in discovery flow ([bd2f40d](https://github.com/FerroxLabs/wayland-core/commit/bd2f40d23aa98d64aff2406f5e7d6b8b45a304ba))
* **mcp:** don't caret-escape the program name in Windows stdio launch ([371f619](https://github.com/FerroxLabs/wayland-core/commit/371f619ee47f1c9beb8d4b984c6f8acc979ce132))
* **providers:** drop unsigned thinking blocks when building Anthropic messages ([cdd0968](https://github.com/FerroxLabs/wayland-core/commit/cdd0968dc66acf53471748ebdd40c460b2630b3c))
* **providers:** make MiniMax visible in pickers + bound tool-input accumulator ([e8ac0f2](https://github.com/FerroxLabs/wayland-core/commit/e8ac0f29642e75a97143ec73d9172cb185f5eb1a))


### Build System

* **release:** prepare 0.12.1-rc.2 prerelease ([93975b7](https://github.com/FerroxLabs/wayland-core/commit/93975b72dfa485896e336181dabb85d858d052a6))

## [0.12.1-rc.1](https://github.com/FerroxLabs/wayland-core/compare/v0.12.0...v0.12.1-rc.1) (2026-06-17)


### Features

* **agent:** allow chatgpt.com egress when the chatgpt provider is active ([b3372ac](https://github.com/FerroxLabs/wayland-core/commit/b3372ac8af6b639934b293e0915e21d0c604aebb))
* **agent:** wire openai-chatgpt provider with oauth bearer source ([18a50d6](https://github.com/FerroxLabs/wayland-core/commit/18a50d626b45f8bc78ef729f6836732193f9a971))
* **channels,tui:** surface channel integrations in /doctor + fix F-019 (S10 v1) ([6958c1c](https://github.com/FerroxLabs/wayland-core/commit/6958c1cfbb11e648166af0571c3b42772339584f))
* **cli:** wayland auth login/logout/status for chatgpt ([060dc45](https://github.com/FerroxLabs/wayland-core/commit/060dc4533e6df3781a0fefb8021c31500fa5ecd8))
* **config,tui:** redacted effective-config preview (S9 v1) ([ff30d20](https://github.com/FerroxLabs/wayland-core/commit/ff30d2051303c85cf1019951b59cfccc7cc8287b))
* **config:** chatgpt_defaults compat preset ([8fac871](https://github.com/FerroxLabs/wayland-core/commit/8fac87162af5dd40c9f26c0a7b2196d1590aca55))
* **config:** config cockpit — paste-to-connect, editors, /doctor health, /effective, channels, discovery ([8fe5559](https://github.com/FerroxLabs/wayland-core/commit/8fe5559f04131ea02a0ffba23402f5a36a76f6df))
* **config:** connected_providers credential helper ([4cffba9](https://github.com/FerroxLabs/wayland-core/commit/4cffba9030a56ad6d7c4fdedf08bf80a5060414c))
* **config:** openai-chatgpt provider type + parsing ([5709f87](https://github.com/FerroxLabs/wayland-core/commit/5709f87ae5de3e1633b4f6cf6141e9213a70627d))
* **config:** read the Forge local-MCP discovery file (Slice 3) ([1014e21](https://github.com/FerroxLabs/wayland-core/commit/1014e212eab7bf472f4ac38c02fe9939c2116cc4))
* **mcp:** /mcp connect — one-command zero-config Forge MCP connect (Slice 3, Piece 3) ([17973e6](https://github.com/FerroxLabs/wayland-core/commit/17973e6bbae98189aeefacd4bdc798e55bbf8b3a))
* **mcp:** DISCOVERED row-to-connect + boot-hero Forge line (Slice 3b polish) ([509fd69](https://github.com/FerroxLabs/wayland-core/commit/509fd69a9d3e14ca5211cfbe04b4d559f7c92db8))
* **mcp:** Forge connect flow — ${cred:KEY} headers + live token grant (Slice 3) ([3f66b9f](https://github.com/FerroxLabs/wayland-core/commit/3f66b9f0457bf11c5f66fd9519c016639c6a8952))
* **mcp:** Forge connect polish — selectable DISCOVERED row + boot-hero line (Slice 3b) ([d19af5b](https://github.com/FerroxLabs/wayland-core/commit/d19af5bf85dc1271dd736a53f7e5f8b3701c1289))
* **mcp:** Forge loopback grant client — liveness probe + scoped token (Slice 3) ([df9d1c9](https://github.com/FerroxLabs/wayland-core/commit/df9d1c9ba8bc4e8f08fb1028cbc0dcd7a246e84a))
* **mcp:** Forge zero-config local-MCP discovery — keystone + reader + grant client + connect flow (Slice 3, headless) ([106b869](https://github.com/FerroxLabs/wayland-core/commit/106b8696412d04ca6f53ded3baab453b5de21f66))
* **mcp:** opt-in allow_local to connect trusted loopback MCP servers ([68b0a6b](https://github.com/FerroxLabs/wayland-core/commit/68b0a6ba4902aea9fcfc578e655fa92ebda38939))
* **oauth:** add ChatGPT device-code login (headless/remote path) ([2a6a4e6](https://github.com/FerroxLabs/wayland-core/commit/2a6a4e69118b1af2d3f06dc98d5613f6608f4fee))
* **oauth:** chatgpt token manager with rotating refresh, JWT account-id decode, and flow descriptor ([9a1b5c1](https://github.com/FerroxLabs/wayland-core/commit/9a1b5c156061515b12bab85da2cba5ecedb4b6e1))
* **oauth:** extra authorize params, configurable redirect host/path with dual-stack loopback bind, id_token capture ([765c11a](https://github.com/FerroxLabs/wayland-core/commit/765c11adb9137c28541dda88529a13fdd596dc28))
* **oauth:** import chatgpt tokens from codex cli ([630688d](https://github.com/FerroxLabs/wayland-core/commit/630688d051a0e6302829efa5edb2821847efefd8))
* **providers:** add key fingerprinting for paste-to-detect config ([e71d8ca](https://github.com/FerroxLabs/wayland-core/commit/e71d8ca1d63a98c0c5890481eae9f7a00053686b))
* **providers:** add live key-validation ladder for paste-to-detect ([c576df9](https://github.com/FerroxLabs/wayland-core/commit/c576df9d6104ec3fc53fb57bfe8fb035d16fa82d))
* **providers:** live Bedrock model discovery via ListFoundationModels ([27a25dc](https://github.com/FerroxLabs/wayland-core/commit/27a25dcb0e533eaab1a67ca6bc79224a626b7ff6))
* **providers:** live Gemini model discovery ([ed2126e](https://github.com/FerroxLabs/wayland-core/commit/ed2126e6410fa39f26c575e86308dca5c1119f98))
* **providers:** make runtime provider construction OAuth-aware for openai-chatgpt ([3e067c1](https://github.com/FerroxLabs/wayland-core/commit/3e067c1a414a37a9d4df70c3d44ecb7ca176e257))
* **providers:** ModelCatalog.refresh_connected live discovery service ([0bc02bc](https://github.com/FerroxLabs/wayland-core/commit/0bc02bce82c4c1529f36fcd50138050226b9c237))
* **providers:** openai-chatgpt provider over async oauth bearer source ([c19a795](https://github.com/FerroxLabs/wayland-core/commit/c19a795fde0dfa833e6463f7df66d3816fd465d6))
* **providers:** orchestrate paste-to-detect (fingerprint + validate) ([804373e](https://github.com/FerroxLabs/wayland-core/commit/804373ef44a94af336bc1f3ebca8174cc871f14e))
* **providers:** per-provider model-list disk cache (24h TTL) ([785704e](https://github.com/FerroxLabs/wayland-core/commit/785704ec5d8dbf3d854712187ca7d3ec7975ec5e))
* Sign in with ChatGPT (OpenAI Codex OAuth) ([5ccc0fc](https://github.com/FerroxLabs/wayland-core/commit/5ccc0fcc48ecf1ccc7203277375c853069cf08c8))
* **tui:** /model picker reads live cached models + refreshes on open ([f94e2c0](https://github.com/FerroxLabs/wayland-core/commit/f94e2c02561b6b9812b56ff3faede7547394d9f6))
* **tui:** Advanced config tier — observability/storage/security editors (S6) ([94dc918](https://github.com/FerroxLabs/wayland-core/commit/94dc9182c22de94cf9bfe589f9ccce5dec2cc447))
* **tui:** arrow-key /model and /provider pickers (cross-provider) ([4b46606](https://github.com/FerroxLabs/wayland-core/commit/4b466061e4073a5a8443948cb512086998ff844a))
* **tui:** boot-screen provider discovery + Tab always switches tabs (FIX-5, FIX-7) ([b7f03d9](https://github.com/FerroxLabs/wayland-core/commit/b7f03d906b011f0cc12cf2118a6abe109c18fac8))
* **tui:** collection list editors — tools/egress/failover (S7) ([299cdb7](https://github.com/FerroxLabs/wayland-core/commit/299cdb7432eddcf4162115bcd859f60473a8f0e1))
* **tui:** config-posture health section in /doctor (S8) ([4f1cb34](https://github.com/FerroxLabs/wayland-core/commit/4f1cb345fb4ab0b74710d823ab09a24620caf07d))
* **tui:** Essentials config home — Tools + Wallet rows, posture + health/cost (S5) ([fbaa431](https://github.com/FerroxLabs/wayland-core/commit/fbaa431d31beed947aad16869b511480323bf127))
* **tui:** make /provider picker connection-aware ([130bc72](https://github.com/FerroxLabs/wayland-core/commit/130bc7288d8c9522bae46b34a16a1ed98a18ca9e))
* **tui:** open the command palette with / from any surface ([2f21d06](https://github.com/FerroxLabs/wayland-core/commit/2f21d0688a71e0e956bc3d108a9bf6a9ef4f6fad))
* **tui:** paste-to-connect door in the Config Providers tier (FIX-3) ([e16f293](https://github.com/FerroxLabs/wayland-core/commit/e16f293abb407d7dac1d8a21a62159c9dd14d22f))
* **tui:** paste-to-detect modal state machine + view-model (S4a) ([6cb6e25](https://github.com/FerroxLabs/wayland-core/commit/6cb6e250425ee521177f88aeb3ad695bed628187))
* **tui:** self-configure discovery section in /doctor (S11 v1) ([f01c9f9](https://github.com/FerroxLabs/wayland-core/commit/f01c9f940b1f8448bc054f10475df98e3feeda94))
* **tui:** wire the paste-to-detect /connect overlay (S4b) ([7b75549](https://github.com/FerroxLabs/wayland-core/commit/7b75549b8c2120c247dc6940cd5a840af5a01dd1))
* **types:** codex model aliases for openai-chatgpt ([daa6210](https://github.com/FerroxLabs/wayland-core/commit/daa6210a5ded3e1d95015ab1a0c195cbc9d18cca))


### Bug Fixes

* **model-catalog:** tag a floored model fetch BuiltIn, not a live "synced" ([0bca1a7](https://github.com/FerroxLabs/wayland-core/commit/0bca1a7545c8a5e4d8e7fa155e63f1e694d3014c))
* **model-picker:** load UI-saved provider keys + connection-aware live /model picker ([3a8929f](https://github.com/FerroxLabs/wayland-core/commit/3a8929fd45e9c5ef26ddabe79cf1904d570fd931))
* **providers:** accept codex response.done/incomplete as terminal frames ([0bc0ed6](https://github.com/FerroxLabs/wayland-core/commit/0bc0ed62a96ef8048c67e8a56e962a1ed8f93cff))
* **providers:** Bedrock/Vertex "connected" only with real ambient credentials ([7245065](https://github.com/FerroxLabs/wayland-core/commit/72450658c87fb78c642a91b54ce041f5dcf7cc1d))
* **providers:** don't request encrypted reasoning until we round-trip it ([52eeceb](https://github.com/FerroxLabs/wayland-core/commit/52eecebb3ae3ea70caa4d074a1b4cc68b9890ef4))
* **providers:** drop unused json import; lock socket2/base64 direct edges ([fd9100e](https://github.com/FerroxLabs/wayland-core/commit/fd9100ec250b2cc674887ed47d2cb48e437f5ff6))
* **providers:** forward list_models on OpenAI-compat newtypes (paste-connect) ([efbddba](https://github.com/FerroxLabs/wayland-core/commit/efbddba218df0f854f914a7ee77ff9e4b2fd324d))
* **providers:** ResilientProvider delegates alias_key/list_models to primary ([4c409c1](https://github.com/FerroxLabs/wayland-core/commit/4c409c1da6e5506c615a9279cbd092f41bcb56fe))
* **tui:** Config Esc saves pending toggles instead of reverting ([854f065](https://github.com/FerroxLabs/wayland-core/commit/854f0657843aee2ce2b4af0e0029adfedec45d62))
* **tui:** show em-dash for unrecorded spend in the status bar ([f8e5d65](https://github.com/FerroxLabs/wayland-core/commit/f8e5d6540a370d3a3398161c2e15437da3127f85))
* **tui:** stop /doctor from freezing the whole TUI on live probes ([4121652](https://github.com/FerroxLabs/wayland-core/commit/4121652ebd66cae28084d67d3d64ea6107da020c))
* **tui:** widen Advanced label pad so the value isn't glued to it ([1cb6578](https://github.com/FerroxLabs/wayland-core/commit/1cb65780e38e374606454eea865d520b20798087))


### Documentation

* **providers:** document Sign in with ChatGPT ([90e0c62](https://github.com/FerroxLabs/wayland-core/commit/90e0c6216347e4da8ae068729e7dd1b7104d093c))


### Build System

* **release:** prepare 0.12.1-rc.1 prerelease ([9c5922b](https://github.com/FerroxLabs/wayland-core/commit/9c5922b12b9fe35ba5636421619b756043a596ab))

## [0.11.0-rc.1] - 2026-06-11

Release candidate for 0.11.0. The headline is **inbound channels** — Wayland Core now receives, not just sends — plus native per-command Bash output compaction, a JWT crypto-backend security fix, and a batch of provider and platform fixes. Still a public beta; cut as an RC to soak the new network-facing channel surface before the final 0.11.0.

### Highlights

* **Inbound channels.** Two-way messaging across Telegram, Discord, Slack, WhatsApp, Matrix, Microsoft Teams, and SMS: inbound receive (long-poll / `/sync` / webhook host), an engine-backed turn dispatcher with a tool-posture scope for channel-originated agents, reconnect supervision so channels survive disconnects, Microsoft Teams Bot Framework JWT validation, outbound chunking with per-platform size caps, an idempotency nonce to dedupe retried sends, and react/typing with ack reactions + a typing keepalive state machine.
* **Auth-aware inbound media.** Images and audio attachments are fetched and described/transcribed before the turn, with credentials kept inside each connector boundary.
* **Native Bash output compaction.** Verbose `cargo` / `git` / test-runner / `grep` output is compacted into the model's transcript (the human still sees full output) — block-aware, fail-open, size-gated, default-on via `ProviderCompat::compact_bash`, with per-call savings telemetry.
* **Security.** Migrated the JWT crypto backend to `aws_lc_rs`, dropping `rsa` and eliminating RUSTSEC-2023-0071 (Marvin Attack) at the source. Closed a Grep RCE, skill/rules prompt-injection, and hook shell-execution hardening; capped stdin line length (newline-less OOM DoS); fail-closed on UTF-8 split-codepoint corruption.

### Providers

* gpt-5 family now routes to the OpenAI Responses API (`/v1/responses`).
* Gemini 2.5-class: split SSE frames on CRLF (stops false truncation); inject default items for array schemas (stops tool-registration 400s).
* Default moonshot/qwen to their international endpoints; pin `api_path` so 8 native providers stop 404ing.

### Fixes

* ALSA is no longer a hard dependency — `cpal` is gated behind an off-by-default `voice` feature, so the default binary runs on minimal Linux without `libasound` (#14).
* The `/config` providers pane now scrolls to keep the focused row visible on short terminals (#16).
* PATHEXT-aware `npx` detection on Windows so the IJFW MCP server registers (#6).
* Legacy-YAML migration no longer clobbers an existing `config.toml`.

### Extensibility

* Declarative on-disk plugins under the profile home, wiring hooks + MCP into the engine.

## [0.10.0] - 2026-06-08

First public release. Wayland Core is a domain-agnostic autonomous-agent engine written in Rust: terminal-first, multi-provider, MCP-native, and embeddable. It ships as a **public beta**, capable and open, and still hardening under a continuous endurance soak (see "Built to endure" in the README).

### Highlights

* **Multi-provider.** 7 native provider integrations (Anthropic, OpenAI, Google Gemini, Google Vertex AI, AWS Bedrock with SigV4, Cohere, Azure OpenAI) plus a 104-entry models.dev catalog, all behind one provider-neutral engine and a declarative ProviderCompat layer. Circuit-breaker resilience, mid-stream reconnect, and multi-key rotation across every API-key provider.
* **Orchestration.** Sub-agents, a git-worktree-isolated parallel swarm with a dirty-tree guard, declarative ForgeFlows workflows that lower onto the engine's own execution graph, and selectable reducers via `wayland swarm --reduce mesh|fleet|consensus|debate`.
* **Security by default.** A fail-closed OS-native sandbox (bubblewrap, sandbox-exec, AppContainer), a CI-enforced egress chokepoint with an exfil-shape classifier, an always-on SSRF and metadata floor, and argv-safe shell execution.
* **Extensibility.** MCP in both directions (a client, and a server that advertises and executes its own built-in tools, with runtime injection), roughly 70 built-in tools, skills, blocking lifecycle hooks, and a plugin API.
* **Embeddable.** A typed JSON-Lines protocol drives the engine headlessly behind a host app.
* **Self-evolution (GEPA).** A scored optimizer that evolves prompts and skills against your own reference cases.

### Surfaces

One binary, three ways to run it: a one-shot command, an interactive TUI, or a headless JSON stream.

### Notes

This is a public beta. APIs and behavior may change before 1.0. A continuous, fault-injected endurance trial is ongoing; the method, measurements, and honesty bounds are documented in [docs/resilience.md](docs/resilience.md).
