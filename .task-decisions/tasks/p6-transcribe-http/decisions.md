# Decisions — p6-transcribe-http

### D1 · Active · Remaining work order: HTTP, CPU speed, downloader, pumice command, formatter
- date: 2026-10-15
- decided by: brasillero
- origin: reports/2026-10-15-implementation-order.md
- area: process
- revisit if: —

**Decision.** Transcription over HTTP comes next, then runtime CPU selection, the model downloader, the `pumice` entry point with `relay.yml`, and the formatter with the combined flow. GPU moves up right after CPU speed if latency is still unacceptable; replacing Symphonia comes later.

**Why.** The owner ranks transcription over HTTP highest; at about 19 s per 14 s clip, CPU speed is the next limit.

**Rejected.** `arch-modules/D11`'s order, with the orchestrator before more transcription work; GPU right after HTTP.

### D2 · Active · The downloadable package for pumice-transcribe is not scheduled
- date: 2026-10-15
- decided by: brasillero
- origin: conversation
- area: distribution
- revisit if: the owner asks for a downloadable pumice-transcribe

**Decision.** The release archive and the Windows/macOS engine builds for `pumice-transcribe` (`p4b-transcribe-config#2`) are not scheduled; it is built from source for now.

**Why.** Transcription over HTTP matters more to the owner now.

**Rejected.** Doing the package next, as `p4b-transcribe-config#2` planned.

### D3 · Active · A busy transcription server answers 429 at once
- date: 2026-10-15
- decided by: brasillero
- origin: reports/2026-10-15-implementation-order.md
- area: api
- revisit if: the endpoint is exposed to other people

**Decision.** One transcription runs at a time; a request arriving while one runs gets HTTP 429 immediately, with no waiting line.

**Why.** Simplest and safest while only server-1 uses it.

**Rejected.** A short waiting line.

### D4 · Active · Silence returns empty text
- date: 2026-10-15
- decided by: brasillero
- origin: reports/2026-10-15-implementation-order.md
- area: api
- revisit if: —

**Decision.** When transcription succeeds with no text, the endpoint answers 200 with `{"text": ""}`; the client decides what to do.

**Why.** An empty transcript is a valid answer.

**Rejected.** An `empty_output` error.

### D5 · Active · The served model stays loaded while the server runs
- date: 2026-10-15
- decided by: brasillero
- origin: reports/2026-10-15-implementation-order.md
- area: transcription
- revisit if: —

**Decision.** `serve` loads one model at startup and keeps it in memory (about 1 GB for Turbo Q8_0).

**Why.** Fast answers; server-1 has enough memory.

**Rejected.** Loading the model only when a request arrives.

### D6 · Active · Endpoints match the OpenAI API as a requirement
- date: 2026-10-15
- decided by: brasillero
- origin: conversation
- area: api
- revisit if: —

**Decision.** Every Pumice HTTP endpoint must match the OpenAI-compatible API standard (request fields, response shapes, error envelope, status codes) as a requirement. Unsupported OpenAI options are rejected explicitly, never silently ignored.

**Why.** Apps such as T3 must integrate without adapters.

**Rejected.** A Pumice-specific contract, or "close enough" compatibility.

**Consequences.** Reviews check the endpoint against the official OpenAI API reference; Kimi and Codex iterate until Codex approves, then the supervisor does a full final review.

### D7 · Active · Transcription accepts Opus audio
- date: 2026-10-15
- decided by: brasillero
- origin: #4
- area: transcription
- revisit if: —

**Decision.** The transcription module adds support for Opus audio (WebM/Opus and Ogg/Opus), the format browsers record in.

**Why.** Apps like T3 that record in the browser send WebM/Opus; without it they need a conversion step.

**Rejected.** Leaving Opus out until Symphonia is replaced.

### D8 · Active · The transcription server is not exposed to other people
- date: 2026-10-15
- decided by: brasillero
- origin: conversation
- area: distribution
- revisit if: the owner asks to let friends use transcription

**Decision.** `pumice-transcribe serve` stays on `127.0.0.1` on server-1 only; no tunnel, public hostname or access for friends.

**Why.** Owner choice.

**Rejected.** Exposing it through the Cloudflare tunnel.

### D9 · Active · Kimi k2.8-code writes the code
- date: 2026-10-15
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: the owner evaluates Kimi k2.8-code against k2.7-code-highspeed

**Decision.** Implementation tasks go to Kimi `kimi-k2.8-code` instead of `kimi-k2.7-code-highspeed`; Codex still reviews and the supervisor still does the final review.

**Why.** k2.7-code-highspeed needed many review rounds; the owner wants to try the stronger model and evaluate it later.

**Rejected.** —

### D10 · Active · The supervisor decides and merges while the owner is away
- date: 2026-10-15
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: the owner returns and reviews the work

**Decision.** While the owner is away, the supervising agent orchestrates the plan in D1 (with D7 and D8), decides open questions on the owner's behalf, and merges pull requests when needed. Decisions taken that way are recorded as `decided by: claude (orchestrator), brasillero (authorized)`.

**Why.** Owner instruction: keep progressing without waiting for them.

**Rejected.** —
