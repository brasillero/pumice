# Automatic initial settings: implementation and validation

Implementation: `9fb4b64`.

## Behavior

When a command requires a per-user settings file that is absent, it creates
the starting set in that per-user directory before loading and validating
its own file:

```text
relay.yml
transcribe.yml
formatter.yml
orchestrator.yml
example-formatter.clean.yml
```

The shared `pumice-settings` utility embeds the repository's supplied examples
and sample formatter. Standalone modules and the orchestrator call the same
code; neither sidecar examples nor an installer runtime are needed. The
formatter definition is published before its config, and the orchestrator
config follows the module files it references.

Each file is fully written to a private temporary file in the destination
directory and published without replacing an existing entry. Simultaneous
first starts preserve complete files. New directories and files are private
on Unix (0700 and 0600); existing permissions and contents are unchanged.
Creation failures retain the destination path and filesystem reason.

If the requested module file already exists, initialization performs no
writes, including when other modules' settings are absent. Malformed files,
directories and dangling symlinks are preserved and normal validation still
reports their problems. Settings omitted from an existing document remain
errors. Missing explicit `--config` paths remain errors and never trigger
per-user creation. Bare command overviews, help, version and optional
diagnostic reads do not create settings.

All file transcription still reads a complete settings file, including
`--model-file`. A fresh default-path invocation now creates that file before
reading it. Flags override the values read from it. No model is downloaded,
no AI CLI is installed or logged into, and existing module enable settings
retain their meaning. The supplied relay example still enables Claude and
the supplied orchestrator example still leaves `serve.enabled: false`.

## Validation

- `cargo test --workspace -q`: 1,258 passed, no failures, including the
  managed transcription harness's 19 fake-engine tests.
- `cargo clippy --workspace --all-targets -- -D warnings` passed.
- Formatting, `git diff --check`, all four YAML templates and task-note
  shape validation passed.
- Third-party notices (21 tests) and portable release packaging (9 tests)
  passed. The notices generator recognizes the new utility as workspace code.
- Fresh module/orchestrator loads, complete sample-formatter registration,
  second starts, incomplete existing files, missing explicit paths, optional
  diagnostics, symlink preservation, write failures and simultaneous first
  starts are covered in temporary directories.
- The fake-engine one-off file test transcribed successfully after creation,
  using the generated Portuguese/CPU settings, then rejected an incomplete
  file without repairing it. No real inference or paid CLI call was used.

Installation guides, module guides, example comments, archive/npm instructions
and the unreleased changelog now describe automatic creation. Live services,
installed binaries and per-user settings were not changed for this task.
