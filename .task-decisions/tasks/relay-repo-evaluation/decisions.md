# Decisions — relay-repo-evaluation

### D1 · Active · Pumice ships as a CLI for Windows x64, Linux x64 and macOS Apple Silicon
- date: 2026-11-11
- decided by: brasillero
- origin: reports/2026-11-11-repository-evaluation.md
- area: distribution
- revisit if: someone asks for an Intel Mac build

**Decision.** Pumice is a command-line tool available on Windows, Linux and Apple Silicon Macs. Intel Macs are no longer a release target.

**Why.** The owner's main goal for now is a CLI on these three platforms. The Intel build is the slowest job, its GitHub runner image ends in August 2027, Rust demoted the target, and no download shows a user.

**Rejected.** Keeping the Intel target on its dedicated runner, or building it on the Apple Silicon runner: the owner does not want Intel Macs supported.

**Consequences.** The `x86_64-apple-darwin` build, the `darwin-x64` npm package, their smoke test, notices and documentation go (#2). The npm front package lists three native packages.

### D2 · Active · Remove the generic adapter and complete npm delivery for the three platforms
- date: 2026-11-11
- decided by: brasillero
- origin: conversation
- area: distribution
- revisit if: the owner requests different platforms or distribution channels

**Decision.** Delete the archived generic adapter. Update AGENTS.md to name the architectures in D1. Review the remaining findings against relay-only, cancel obsolete work, and proceed with commits and a PR for a CLI installable through npm or npx on Windows, macOS and Linux.

**Why.** The owner explicitly approved deletion and the rules edit, and confirmed npm/npx delivery on the three platforms as the goal.

**Rejected.** Retaining the unused adapter; cancelling findings solely because they were reported before the cleanup without checking current code.

**Consequences.** Native binaries remain necessary for npm/npx. This does not authorize publication, deployment, a version choice or changes to tunnel access policy.
