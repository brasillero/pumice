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

### D11 · Active · Transcription comes right after the workspace, before the relay migration
- date: 2026-10-12
- decided by: brasillero
- origin: #1
- area: process
- revisit if: —

**Decision.** Work order: the Cargo workspace with the relay as its own crate first, then the CPU transcription CLI, then `relay.yml` and the `pumice` orchestrator entry point, then the remaining phases of `docs/research/architecture-plan.md`.

**Why.** Usable transcription sooner; the transcription crate does not depend on the relay migration.

**Rejected.** The plan's order, with the relay config and orchestrator migration (P2, P3) before transcription (P4).

### D12 · Active · No backward compatibility for the command and config changes
- date: 2026-10-12
- decided by: brasillero
- origin: #1
- area: config
- revisit if: Pumice gets users beyond the owner's own setup

**Decision.** The move to `pumice` as orchestrator and `relay.yml` ships without a compatibility window, legacy aliases or a `config migrate` command: nobody but the owner uses the configuration yet.

**Why.** Owner: no one is using it yet, so compatibility code is wasted effort.

**Rejected.** The plan's 0.3–0.4 compatibility window with removal in 0.5, and `pumice config migrate`.

**Consequences.** The server-1 service unit and its `pumice.yaml` are updated by hand when the new version is deployed there. Handy's URL and port 7567 still have to keep working for the owner's friends.

### D13 · Active · `pumice` runs relay and formatter in-process and transcription as a separate program
- date: 2026-10-12
- decided by: brasillero
- origin: #1
- area: architecture
- revisit if: —

**Decision.** Hybrid dispatch: the relay and formatter are libraries built into the `pumice` executable, which stays one static file; transcription is a separate program (`pumice-transcribe` with its native libraries and GPU backends) that `pumice transcribe` launches.

**Why.** Keeps the core a single static file and npm unchanged, while transcription's native code and DLLs stay isolated from the relay.

**Rejected.** Every module as a separate program launched by `pumice`: the core would no longer be one file.

### D14 · Provisional · Modules talk to each other over loopback only
- date: 2026-10-12
- decided by: brasillero
- origin: #1
- area: architecture
- revisit if: the first iteration is done and remote modules are taken up

**Decision.** In the first iteration, modules communicate only over `127.0.0.1`. The final goal is to allow remote modules (for example transcription on another machine).

**Why.** Remote modules need a network and authentication policy first.

**Rejected.** Remote modules in the first iteration.

**Consequences.** `AGENTS.md`'s network rule needs a loopback allowance between modules.

### D15 · Active · The `s10.1-transcription` spike closes unmerged and its branch stays as evidence
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: conversation
- area: transcription
- revisit if: —

**Decision.** Draft PR #91 (`s10.1-transcription`) is closed without merging. The remote branch is kept, and its task notes and research note are not brought to `main`.

**Why.** The spike answered its question: `transcribe-cpp` 0.3.1 and the four-target static CPU build were adopted, and the transcription module was built in P4–P9 (`crates/transcription`). Its endpoint design inside the relay and its AVX2-only policy were superseded by D1 and D8. `docs/research/architecture-plan.md` cites the branch at `e644902` as evidence, so deleting it would break those references.

**Rejected.** Merging the spike: its standalone crate, CI workflow and notes describe a design that no longer holds. Deleting the branch: it breaks the evidence citations.

**Consequences.** The owner asked the orchestrator to decide (2026-10-15). PR #91 was closed with a comment explaining this.
