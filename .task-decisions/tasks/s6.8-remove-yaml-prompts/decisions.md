# Decisions — s6.8-remove-yaml-prompts

### D1 · Active · Pumice takes no prompts from its config
- date: 2026-10-08
- decided by: brasillero
- origin: conversation
- area: config
- revisit if: —

**Decision.** The YAML `prompts:` section is removed; a file that still has it fails at that line. The app sends its own prompt.

**Why.** Imported from `HANDOFF.md`.

**Rejected.** —
