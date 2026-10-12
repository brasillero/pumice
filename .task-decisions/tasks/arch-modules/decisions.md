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

### D6 · Active · The bare `pumice` command is the orchestrator and runs any part or the whole stack
- date: 2026-10-12
- decided by: brasillero
- origin: #1
- area: cli
- revisit if: —

**Decision.** `pumice` becomes the orchestrator now. Its CLI runs a single module (relay, transcription, formatter) or the whole stack.

**Why.** Owner choice: one entry point for the product, with each part still runnable on its own.

**Rejected.** Keeping bare `pumice` as the relay with the orchestrator under `pumice orchestrate` (the analysis' recommendation): it protects today's commands but does not match the product the owner wants.

**Consequences.** Today's users (the server-1 service running `pumice --plain`, npm installs, docs) need a migration path to the relay subcommand; Handy's URL and port 7567 keep working.

### D7 · Active · Each module has its own config file, the relay's is `relay.yml`
- date: 2026-10-12
- decided by: brasillero
- origin: #1
- area: config
- revisit if: —

**Decision.** Configuration is one YAML file per module: `relay.yml` for the relay, plus files for transcription, the formatter and the orchestrator, which refers to the others.

**Why.** Each module stays usable alone and owns its settings.

**Rejected.** One big file with a section per module.

**Consequences.** Today's `pumice.yaml` becomes the relay's `relay.yml`; existing installs need a migration (for example reading the old name with a warning).

### D8 · Active · Transcription ships CPU, runtime CPU selection and GPU
- date: 2026-10-12
- decided by: brasillero
- origin: #1
- area: transcription
- revisit if: —

**Decision.** The transcription module supports a plain CPU build, automatic runtime selection of the best CPU variant, and GPU backends.

**Why.** D2 allows multiple files; the owner wants the best possible solution rather than one fixed CPU level.

**Rejected.** A single AVX2-only build (`s10.1-transcription/D5`), and GPU-only work that skips the CPU path.

**Consequences.** Supersedes `s10.1-transcription/D5` (AVX2-only releases); that task's notes must be updated to match.

### D9 · Active · Transcription may download models
- date: 2026-10-12
- decided by: brasillero
- origin: #1
- area: transcription
- revisit if: —

**Decision.** The transcription module may download models, as an exception to the rule that Pumice makes no outbound calls besides the AI CLIs. Local model files stay supported.

**Why.** Easier setup, and a future UI will need to offer model choice and download.

**Rejected.** Local model files only.

**Consequences.** `AGENTS.md`'s network rule needs this exception written in. Downloads should be explicit, pinned (revision and sha256) and never happen implicitly at startup.

### D10 · Active · Module CLIs and contracts are designed for a future UI
- date: 2026-10-12
- decided by: brasillero
- origin: D4
- area: architecture
- revisit if: —

**Decision.** Every module's CLI and API is designed so a UI can drive it later: machine-readable output, progress and status, model listing and downloads, without the UI duplicating module logic.

**Why.** Owner: "always keep in mind that one day we will have a UI".

**Rejected.** —
