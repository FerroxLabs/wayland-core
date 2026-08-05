# F16 Provider Stream Cancellation Proof

Status: the provider worker-cancellation slice of F16 is implemented and sealed
by the commit containing this document. F16 remains open for its other
independently releasable reliability slices.

Base: `108792ef56056c74251d697820de9da3717bf529`

## Failure and fix

The engine already selects user cancellation against both the initial provider
future and the returned event receiver. Dropping that receiver promptly ended
the run, but a spawned HTTP response worker could remain parked in
`bytes_stream().next()` until the five-minute between-bytes timeout. The worker
could not deliver useful output after cancellation, yet retained its task,
connection, and response state.

All spawned provider byte-stream readers now select the next network chunk
against `mpsc::Sender::closed()`. Receiver closure wins a simultaneous race and
returns a clean worker exit. Normal items, end-of-stream handling, truncation
errors, terminal events, and the existing network timeouts remain unchanged.

The shared polling boundary covers:

- OpenAI Chat Completions and Responses, including OpenAI-compatible, Azure,
  Flux, and ChatGPT callers;
- Anthropic-compatible streaming, including its Vertex caller;
- Gemini and Vertex Gemini streaming;
- Cohere streaming; and
- Bedrock's AWS event stream.

Bedrock's buffered invocation path is intentionally unchanged. It runs inside
the provider future before a receiver is returned, so the engine's existing
provider-future cancellation drops it directly rather than leaving a spawned
worker.

## Verification

Authoritative Cargo execution ran on the Linux Hetzner proof harness from the
staged tree; Cargo was not run on macOS except `cargo fmt`.

- A pending-stream unit regression proves receiver drop wins within 100 ms.
- Item and ordinary end-of-stream regression paths remain distinct.
- A real local TCP server sends valid streaming response headers and then
  withholds the body indefinitely; dropping the OpenAI event receiver makes the
  actual response worker exit within 250 ms rather than awaiting read timeout.
- `cargo nextest run -p wcore-providers --no-fail-fast`: 942 passed, 0 failed,
  0 skipped.
- Strict all-target/all-feature clippy and staged-tree format/diff checks are
  required before the seal.

## Remaining boundary

This closes the provider worker leak after run cancellation. The configured
connect and between-bytes deadlines remain the independent backstop when the
consumer is live but a provider stalls. Browser sidecar completion and native
packaged-platform regressions remain separate F16 proof obligations.
