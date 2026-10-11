# Decisions — s7.4-restore-npm-scope

### D1 · Active · npm packages are published under the @brasillero scope
- date: 2026-10-05
- decided by: brasillero
- origin: conversation
- area: distribution
- revisit if: —

**Decision.** The front package is `@brasillero/pumice` (installed command `pumice`) and the natives are `@brasillero/pumice-<platform>`.

**Why.** npm rejects the unscoped name `pumice` as too similar to `juice`. Imported from `HANDOFF.md`.

**Rejected.** The unscoped `pumice` (rejected by the registry); `@bresillero` (a misspelled account). The owner may pick a final public name.
