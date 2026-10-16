# Draft upstream report for process-wrap — 2026-10-16

Not filed. Drafted for the owner to post at https://github.com/watchexec/process-wrap/issues if they want to (filing publishes it, so the orchestrator leaves it to the owner).

**Title:** Windows JobObject: a child created suspended can be orphaned outside the job, still holding the parent's inherited handles

**Problem.** With process-wrap 10.0.1 (features `tokio1`, `job-object`, `kill-on-drop`), `JobObject` adds `CREATE_SUSPENDED`, then in `wrap_child` runs `make_job_object` → `AssignProcessToJobObject` → `resume_threads`. If the parent is terminated before the assignment, the child stays suspended forever, outside any job, so `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` never applies. Because std spawns with `bInheritHandles=TRUE`, the child also keeps every inheritable handle of the parent, including the parent's own stdout and stderr, so whoever reads the parent's output never sees end of file. Seen on GitHub Actions `windows-latest`: `pumice-transcribe.exe [Suspended]` left behind and the caller hung (Pumice CI runs 38866510968, 38867288787, 38867950120).

**Reproduction idea.** A parent with piped stdout spawns children through `CommandWrap` + `JobObject` + `KillOnDrop` in a loop while a supervisor calls `TerminateProcess` on the parent at random moments; sometimes the supervisor's read of the parent's stdout never ends and a suspended child remains. Deterministic variant: a parent that creates a child with `CREATE_SUSPENDED` and never assigns it, then is killed.

**Proposed fix.** On Windows 10+, create the child inside the job atomically with `PROC_THREAD_ATTRIBUTE_JOB_LIST`, removing the suspend–assign–resume sequence. std exposes this only through `CommandExt::spawn_with_attributes`, still unstable (rust-lang/rust#114854). Until then: document the window, recommend callers make their own standard handles non-inheritable.

**Environment.** process-wrap 10.0.1 (`tokio1`, `job-object`, `kill-on-drop`, `creation-flags`, `process-group`); tokio 1.53.2; Rust 1.99.0; GitHub Actions `windows-latest`.

**Pumice's own mitigation** (this task): the standard handles are made non-inheritable at startup of `pumice` and `pumice-relay`, so an orphan can no longer pin the caller's pipes. The orphaned suspended process itself can still remain until reboot or manual termination; only the upstream fix removes it.
