# Release 0.4.0 preparation (2026-11-16)

The owner authorized publication and deployment, chose 0.4.0 instead of reusing the modular-product 0.3.0 draft, and explicitly deferred Host/Origin policy changes.

## Changes and evidence

- `Cargo.toml` and the root package entry in `Cargo.lock` now name 0.4.0; dependency versions are unchanged.
- `CHANGELOG.md` records the relay-only release and migration from published 0.2.0 or unpublished modular builds. The 0.3.0 notes are clearly marked as a retired, unpublished draft; released 0.2.0 and earlier sections are untouched.
- npm metadata for the launcher and three supported native packages lists only 0.1.0 and 0.2.0. There was no public `v0.3.0` tag. The obsolete GitHub draft targeted `96de1ee` and carried four modular-product archives. Its metadata was backed up under `/tmp/pumice-release-0.4.0/retired-0.3.0-draft.json`, then the draft was deleted at the owner's request.
- Local validation: 490 Rust tests pass; 76 packaging tests pass (one skipped); fmt, clippy with warnings denied, diff and task-note shape checks pass. Cross-platform release verification runs in CI.

## Publication and deployment execution

After this preparation PR merges, dispatch the manual release workflow on the merge commit, verify both archive and npm checksums, and publish all four npm packages (native packages first) and the GitHub release from that run. The old draft must never supply release files.

The live Linux user service currently runs `/home/brasillero/.cargo/bin/pumice relay serve --plain` with the old `~/.config/pumice/relay.yml`. Migration needs a verified 0.4.0 binary, backup of the existing binary/unit/settings, a preserved copy of the settings named `pumice.yml`, configuration validation, and a unit command ending in `--plain`. Retain port 7567 and provider settings. Test the restarted service with model listing and passthrough only; no paid AI calls are needed. Restore the backed-up binary/unit if readiness fails.

Neither npm nor pnpm is authenticated on this machine. Use the owner's sign-in or a publish credential supplied securely through the secret tool. Never read or copy provider login files. Publication and deployment have not yet happened.

## Questions for the owner

No release-scope questions remain: the owner chose 0.4.0, retirement of the draft, and unchanged access behavior. npm authentication requires owner participation. Host/Origin work is tracked as accepted debt in this task's backlog; it is not a release blocker.
