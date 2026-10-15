# Decisions — p13.1-flow-seam

### D1 · Active · Pumice adds versioned custom endpoints where OpenAI has no equivalent
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-p13-design.md
- area: api
- revisit if: —

**Decision.** `p6-transcribe-http/D6` ("every endpoint matches OpenAI") applies to routes that have an OpenAI contract. Routes with no OpenAI equivalent (dictation, flows, status, managed-child readiness) live under `/pumice/v1/…`, return a versioned JSON body with `schema_version`, and use the OpenAI error object fields (`message`, `type`, `param`, `code`) plus Pumice extensions. A custom response is never presented as an OpenAI response.

**Why.** A dictation (audio in, raw and final text out) has no OpenAI contract; forcing it into `/v1/audio/transcriptions` would break that route's semantics for ordinary clients.

**Rejected.** Overloading `/v1/audio/transcriptions` with a flow field; an unversioned custom route.

### D2 · Active · `pumice serve` exposes only the dictation, flow and status routes
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-p13-design.md
- area: api
- revisit if: a client needs raw OpenAI chat or transcription through the same port as dictations

**Decision.** `pumice serve` serves `GET /health`, `GET /ready`, `GET /pumice/v1/status`, `GET /pumice/v1/flows` and `POST /pumice/v1/dictations`. The plain OpenAI routes (`/v1/chat/completions`, `/v1/audio/transcriptions`, `/v1/models`) are not part of P13; clients that want them keep using `pumice relay serve` and `pumice-transcribe serve`.

**Why.** The UI needs only the dictation route. Re-serving the OpenAI routes would need a strict chat-validation mode in the relay, a model-ID union and byte-exact multipart forwarding (design §5, questions 10–11), which is the riskiest part of the design and not needed yet.

**Rejected.** The design's P13.5 (ordinary endpoint composition) inside P13.

**Consequences.** P13 is four PRs: P13.1 flow seam, P13.2 managed transcription child, P13.3 supervisor and serve shell, P13.4 dictation route and docs. The browser test page follows as P13.5. Admission is a single work permit for dictations only.

### D3 · Active · `pumice serve` listens on 127.0.0.1:17570 by default and never on 7567
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-p13-design.md
- area: config
- revisit if: —

**Decision.** `pumice serve [--config FILE] [--port PORT] [--json]` reads `orchestrator.yml` with a new strict `serve:` section (`enabled`, optional `port` default 17570, `transcription_model`, optional `startup_timeout_ms` default 60000, range 1000–300000). Port 7567 is rejected with a located error; an occupied port fails, with no fallback port; port 0 is allowed only when given explicitly and is announced. Bare `pumice` stays the overview. Files without `serve:` stay valid for `pumice run`.

**Why.** Keeps the live Handy relay on 7567 untouchable and makes the stack opt-in.

**Rejected.** Making `pumice serve` the default command; reusing the relay's port.

### D4 · Active · One warm transcription child serving one model for the service lifetime
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-p13-design.md
- area: architecture
- revisit if: a measurement shows the warm child brings no benefit, or a UI needs several models at once

**Decision.** `pumice serve` owns exactly one `pumice-transcribe serve --managed` child on `127.0.0.1:0`, found like `pumice run` finds it (real-executable sibling), loaded with `serve.transcription_model`. Every enabled flow must use that model (located startup error otherwise). The relay and formatter run in-process (`arch-modules/D13`). Control: lifecycle JSONL on the child's stdout (ready record with instance id, protocol version, package version, port, model, capabilities), stdin EOF means shut down, stderr drained and discarded; managed-only `/ready` and `POST /pumice/v1/transcriptions` (detailed result: text, language, audio seconds, inference time).

**Why.** Avoids per-request process start and model load, and keeps future GPU initialization out of request latency, at the roughly 1 GB resident cost accepted in `p6-transcribe-http/D5`. Measured on server-1 the cached model load is about 0.36 s of a 6 s transcription, so no speedup beyond that is claimed.

**Rejected.** Per-request `pumice-transcribe file` (P12's transport) for serve; several resident models; idle unloading.

### D5 · Active · A crashed or abandoned transcription child is recycled, never replayed
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-p13-design.md
- area: process
- revisit if: —

**Decision.** After readiness, a child exit or three failed 1 s liveness probes (500 ms each; busy inference is healthy) restarts it after 0.5, 1 and 2 s, at most three attempts per episode, reset after 60 s continuously ready; exhaustion leaves the transcription state `failed` until `pumice serve` is restarted. Protocol, version or config errors stop recovery at once; initial startup is not retried. When a dispatched transcription is abandoned (timeout, client disconnect, shutdown) the child is killed, reaped and restarted rather than trusted to cancel. The previous child is always reaped before a new one starts. No request, inference or paid call is ever replayed.

**Why.** Bounded resources without a cancel/acknowledge protocol; no hidden retries of paid work.

**Rejected.** Unbounded restarts; trusting cooperative native cancellation.

### D6 · Active · Dictations take a file and a flow, run under one permit, and keep the raw text on failure
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-p13-design.md
- area: api
- revisit if: —

**Decision.** `POST /pumice/v1/dictations` takes multipart with exactly one non-empty `file` and one text `flow` (enabled flow id, at most 128 bytes), nothing else. One shared work permit, taken before the body is read; busy answers 429 with `Retry-After: 1`, no queue. The response is the P12 schema-1 run envelope; on any failure after transcription the HTTP status is non-2xx, the body keeps the raw transcript and has no `result`. Limits: transcription's 25,000,000 file bytes / 26,000,000 body bytes / 16 KiB per scalar / 120 decoded seconds; 15 s upload; 1 MiB child result; 16 MiB response (too-large final text fails as `relay_invalid_output`, never truncated). The flow deadline is anchored to request arrival, as in `p12-pumice-run/D2`. Status codes as in design §5.

**Why.** Keeps P12's raw/final contract and cost safety over HTTP.

**Rejected.** HTTP 200 with partial results; a request queue; per-request language or prompt overrides (later, explicit extension).

### D7 · Active · Status is polled; no event stream in P13
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-p13-design.md
- area: api
- revisit if: a UI needs the raw transcript before the AI answer arrives

**Decision.** `GET /pumice/v1/status` and `/pumice/v1/flows` are cheap bounded snapshots (no network, CLI or model work, never wait for the work permit). No SSE, WebSocket, jobs or history. `--json` writes bounded lifecycle records to stdout and metadata-only events to stderr through the P12 event sink.

**Why.** The smallest contract a first UI needs.

**Rejected.** SSE stage events and early transcript delivery in P13.

### D8 · Active · The stack accepts only its own loopback origin
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-p13-design.md
- area: api
- revisit if: —

**Decision.** Bind `127.0.0.1` only, no authentication (`arch-modules/D14`). Before admission and before reading the body: Host must be the literal `127.0.0.1:<port>`; a present `Origin` must equal `http://127.0.0.1:<port>`, else 403 (including `null`, `localhost` and other origins); no CORS headers. The browser test page reaches the stack through its same-origin helper, which validates its own origin and strips it before forwarding. No Tailscale or tunnel exposure.

**Why.** Limits DNS rebinding and cross-site form posts against a no-auth local service.

**Rejected.** `Access-Control-Allow-Origin: *`; accepting `localhost` aliases.

### D9 · Active · `pumice serve` shuts down within 5 s and promises no crash isolation for the core
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-p13-design.md
- area: process
- revisit if: —

**Decision.** On Ctrl-C/SIGTERM: stop admission, cancel work, close the child's stdin, 2.5 s grace, then force-kill and reap owned process trees (P12's `ProcessTree`/`CleanupScope`, never a second implementation), exit 130/143 (1 on cleanup failure). The managed child has its own bounded watchdog after stdin EOF. Panics report metadata only and still release resources. A native crash is isolated in the child; a panic in the in-process relay or formatter is a core failure. Windows and macOS teardown stays a manual check (`p12-pumice-run#2`).

**Why.** Same ownership model as `pumice run`, bounded.

**Rejected.** Unbounded drain; claiming full crash isolation.
