# Backlog — release-0.3.0

## Active

### #1 · To do · Owner publishes 0.3.0 to GitHub and npm
- type: Task
- area: distribution
- origin: conversation
- ticket: —
- created: 2026-10-29

**Context.** Nothing publishes automatically: `.github/workflows/release.yml` only builds previews, and npm logins and 2FA are the owner's. The GitHub release carries the four core-only archives and `SHA256SUMS`, as for 0.2.0; transcription ships only in the npm platform packages (`p14.3-npm-bundles/D5`). Publish both from one release-preview run on the merge commit.

**To do.**
- [ ] Merge the release PR; on `main`, run `gh workflow run release.yml --ref main` and wait for it to pass.
- [ ] `gh run download <run-id> -n release-preview -D dist` and `-n npm-preview -D dist/npm`; `cd dist && sha256sum -c SHA256SUMS`.
- [ ] `gh release create v0.3.0 --target <merge-commit> --draft --title v0.3.0` with the four `pumice-0.3.0-*` archives, `SHA256SUMS` and the CHANGELOG 0.3.0 section as notes; review, then publish.
- [ ] `pnpm publish <tarball> --access public` for the four `brasillero-pumice-<platform>-0.3.0.tgz` first, then `brasillero-pumice-0.3.0.tgz` (web login and 2FA in the browser).
- [ ] Smoke: `pnpm dlx @brasillero/pumice@0.3.0 --version` prints `pumice 0.3.0`, and `pumice transcribe --version` prints `pumice-transcribe 0.3.0` on a bundled platform.
- [ ] Record the publication here and finish the task.

**Acceptance criteria.**
- GitHub `v0.3.0` and all five npm packages at 0.3.0 (`latest`).

## Deferred

## Closed
