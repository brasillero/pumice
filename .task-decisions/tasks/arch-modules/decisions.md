# Decisions — arch-modules

### D1 · Active · Pumice is split into independent modules, each with its own CLI
- date: 2026-10-12
- decided by: brasillero
- origin: conversation
- area: architecture
- revisit if: —

**Decision.** Pumice becomes a set of independent modules: the relay (today's Pumice), transcription and a formatter, plus an orchestrator that handles the interactions between them. Each module is a CLI and works on its own.

**Why.** The owner wants to replace Handy eventually, while keeping each piece usable alone and not tied to the others.

**Rejected.** Growing today's relay binary into one program that does everything.

**Consequences.** The `s10.1-transcription` endpoint design, which put transcription inside the relay binary, has to be redone to fit this architecture.

### D2 · Active · The transcription module is independent of the relay and not bound to a single file
- date: 2026-10-12
- decided by: brasillero
- origin: D1
- area: transcription
- revisit if: —

**Decision.** The transcription module is not related to the relay. It does not need to be a single executable: it should use the best possible solution (for example GPU backends or runtime CPU selection), and it is a CLI.

**Why.** The single-file rule came from the relay's original vision as one CLI tool; transcription has different needs.

**Rejected.** Keeping transcription inside the relay's single executable.

### D3 · Active · The formatter injects formatting rules into an incoming request
- date: 2026-10-12
- decided by: brasillero
- origin: D1
- area: architecture
- revisit if: —

**Decision.** The formatter sits between transcription and the relay: it modifies an incoming request to inject the expected formatting rules.

**Why.** Today the client app (Handy) owns the prompt; replacing Handy needs a place that adds it.

**Rejected.** —

**Consequences.** The relay itself stays pass-through.

### D4 · Active · Interfaces are built on top of the CLIs, as in T3 Code
- date: 2026-10-12
- decided by: brasillero
- origin: D1
- area: architecture
- revisit if: —

**Decision.** Any user interface is built over the Pumice CLIs and orchestrator, following T3 Code's pattern of a UI built over its CLI.

**Why.** Owner preference: the modules and their CLIs are the product's core; UIs are layers on top.

**Rejected.** —

### D5 · Active · Codex analyses and reviews, Kimi writes code, Claude supervises
- date: 2026-10-12
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: —

**Decision.** All analysis and investigation is delegated to Codex. Code is written by Kimi (`kimi-k2.7-code-highspeed`), reviewed by Codex, and supervised by the orchestrating Claude session.

**Why.** Owner instruction.

**Rejected.** —
