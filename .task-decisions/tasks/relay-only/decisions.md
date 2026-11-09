# Decisions — relay-only

### D1 · Active · Pumice is the relay only
- date: 2026-11-09
- decided by: brasillero
- origin: conversation
- area: architecture
- revisit if: the owner explicitly requests another product direction

**Decision.** Remove transcription, formatter and orchestration from the product and restore the relay itself as Pumice. Pumice receives the client's OpenAI-compatible chat request, calls the selected official AI CLI, and returns the answer with its existing pass-through behavior.

**Why.** T3 now supplies transcription and post-processing for the owner's intended use. The owner wants Pumice to return to its original relay scope.

**Rejected.** Keeping the modules as optional product features: the owner requested their complete removal.

**Consequences.** This replaces the modular product direction in `arch-modules/D1`–`D4`, `D6`–`D14` and `D16`, and the transcription, formatter and orchestrator features built under it. Historical task notes and Git history describe past work; they do not authorize restoring removed features. Current delivery, docs and project rules must describe the relay alone. Provider, configuration validation and security decisions still apply to the relay.

### D2 · Active · Align AGENTS.md with the relay-only product
- date: 2026-11-09
- decided by: brasillero
- origin: conversation
- area: docs
- revisit if: the owner changes the product scope

**Decision.** The owner authorizes updating AGENTS.md to describe Pumice as relay-only, removing obsolete module rules while preserving provider, security, testing and Git rules.

**Why.** The reserved project rules must describe the product requested in D1.

**Rejected.** Leaving the obsolete rules in place and proposing their replacement only in the PR.
