# Decisions — release-0.4.0

### D1 · Active · Publish relay-only 0.4.0 and retire the modular-product draft
- date: 2026-11-16
- decided by: brasillero
- origin: conversation
- area: distribution
- revisit if: —

**Decision.** Release the relay as 0.4.0, retire the unpublished modular-product 0.3.0 draft, and perform the commits, pushes, publishing and deployment needed to finish delivery.

**Why.** The owner chose a new version instead of reusing 0.3.0.

**Rejected.** Replacing the draft with relay-only 0.3.0.

**Consequences.** Publish archives and three native npm packages plus the launcher from one verified workflow run. Migrate the existing Linux service with backups; preserve provider settings and the loopback port. npm authentication remains an owner step.

### D2 · Active · Release current access behavior and defer Host/Origin restrictions
- date: 2026-11-16
- decided by: brasillero
- origin: conversation
- area: api
- revisit if: the owner supplies allowed hosts and browser origins

**Decision.** Release the current relay access behavior; implement Host/Origin protection separately after deciding tunnel and browser compatibility.

**Why.** The owner selected this option to finish the current release without changing client access.

**Rejected.** Blocking 0.4.0 on a new access policy.

**Consequences.** The browser-triggered quota-use risk remains accepted for this release and is tracked in #1.
