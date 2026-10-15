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
