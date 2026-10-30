# Decisions — ci-speed

### D1 · Active · The agents implement every pipeline speed-up in the report, then compare timings
- date: 2026-10-30
- decided by: brasillero
- origin: reports/2026-10-30-pipeline-speed.md
- area: distribution
- revisit if: —

**Decision.** The owner authorized all eight items (#1–#8), including sharing bundles across workflows (#5) and fewer previews or Intel Mac builds (#8), to be implemented, reviewed and merged by the agents, followed by a before/after comparison report.

**Why.** The owner wants shorter pipelines and trusts the coder, reviewer and supervisor loop ("you guys can do it all").

**Rejected.** Waiting for an owner decision on #5 and #8 after measuring #1–#4.

**Consequences.** #5 and #8 are implemented only in forms that keep what the pipelines prove before a merge, unless the measurements show that impossible; any coverage removed is listed in the comparison report.
