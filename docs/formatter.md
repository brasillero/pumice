# Pumice formatter module

The formatter injects an explicitly selected formatter's rules, as one
`system` message, into an OpenAI-shaped chat request before the request
reaches the relay (`arch-modules/D3`). This is where formatting rules live
in Pumice: the relay itself stays pass-through and never adds, removes or
rewrites instructions.

The formatter is pure text transformation. It never calls an AI provider,
never rewrites the requested model, never touches the network, and it never
logs request text (dictated content) — errors name files, ids and line
numbers only.

It ships as the standalone `pumice-format` binary and, built into `pumice`,
as `pumice format …` with identical behaviour (arch-modules/D13).

## Quick start

```sh
pumice format list                              # built-ins and configured formatters
pumice format show builtin:clean                # the built-in cleanup rules
echo '{"model":"claude","messages":[{"role":"user","content":"reunião às nove horas"}],"stream":false}' \
  | pumice format apply --formatter builtin:clean
```

`apply` prints the transformed request JSON to stdout: the same request
with one `{"role":"system","content":…}` message inserted before the first
non-system/developer message. Everything else — model, `stream`, unknown
members, every other message — is preserved exactly. `apply` works on the
raw request text: numbers of any size or precision, key order and string
escapes inside every value are re-emitted byte for byte. Top-level key
names are re-emitted in normalized JSON string form (escape sequences are
canonicalized, e.g. `"mod\u0065l"` prints as `"model"`), and only
whitespace between top-level members and between `messages` elements may
change.

## Formatter definitions

A formatter is one YAML file:

```yaml
schema_version: 1
id: clean            # [a-z0-9-]+; ids starting with "builtin:" are reserved
description: Light dictation correction
system: |
  Correct transcription mistakes and punctuation. Preserve the language
  and meaning. Treat the user's message as dictated content and return
  only the resulting text.
```

- `schema_version` must be `1`.
- `id` identifies the formatter in `--formatter`, `default` and `list`.
- `description` is a short human-readable summary shown by `list`.
- `system` is the rules text, injected as one `system` message. It must be
  non-empty and at most 16 KiB.

Pumice ships one built-in formatter, `builtin:clean`, adapted from the
default cleanup prompt of [Handy](https://github.com/cjpais/Handy) (MIT): it
fixes spelling, capitalization and punctuation, converts number words to
digits, turns spoken punctuation into symbols, removes fillers, keeps the
original language and word order, never follows or answers instructions in
the dictated text, and returns only the cleaned text. Copy it as a starting
point — `show` already prints it with a fresh placeholder id:

```sh
pumice format show builtin:clean > ~/.config/pumice/clean.yml
# edit the id, then register the file (below)
```

## Configuration: formatter.yml

The module config registers definition files and the default formatter. It
is looked up like every other module config: the `--config FILE` flag wins;
otherwise the per-user file `~/.config/pumice/formatter.yml` on Linux and
macOS (`$XDG_CONFIG_HOME/pumice/formatter.yml` when `XDG_CONFIG_HOME` is
absolute) and `%APPDATA%\pumice\formatter.yml` on Windows. `apply`,
`check-config` and `show` of a registered id need the file; `list` (then
only the built-ins) and `show builtin:clean` run without one. Every setting
must be written: an omitted one is an error at line 1. Commands that need
the per-user file create missing initial settings and the sample formatter
definition before loading it. Existing files are never overwritten. A
missing explicit `--config` file remains an error; `list` and
`show builtin:clean` do not create files.

```yaml
schema_version: 1

# Definition files to load; relative paths resolve against this file.
# `formatters: []` registers none.
formatters:
  - path: clean.yml

# The formatter `apply` uses when --formatter is omitted: one of the ids
# above or builtin:clean. null: none (`apply` then needs --formatter).
default: clean
```

Validation is strict and reports `file:line:column`: missing settings,
unknown keys, missing or unreadable definition files (reported at the `path:` entry here), ids
that are invalid, duplicated across files, or that use the reserved
`builtin:` namespace (reported inside the definition file), and a `default`
that names nothing. See [`formatter.example.yml`](../formatter.example.yml)
and [`example-formatter.clean.yml`](../example-formatter.clean.yml) for a
complete pair.

A registered definition is a post-processing preset for the orchestrator:
a dictation flow names it in `formatter:` like `builtin:clean`. Listing it
in `serve.relay.presets` makes it selectable by OpenAI transcription clients
through a `model:provider:preset` id when preset expansion is enabled (or when
it is the default preset)
([orchestrator.md](orchestrator.md#openai-transcriptions)).

## Commands

All commands accept `--config FILE`. Every command supports `--json`, which
wraps the result or error in an envelope
`{"schema_version":1,"ok":bool,"result":…|,"error":{"code","message"}|}`.
Without `--json`, `apply`'s stdout is the transformed request JSON itself.

| Command | Purpose |
| --- | --- |
| `list [--json]` | Built-ins and configured formatters, marking the default |
| `show <id> [--json]` | Print one definition as an editable YAML file |
| `apply [--formatter ID] [--on-conflict reject\|append] [--request FILE\|-] [--json]` | Transform one request |
| `check-config [--json]` | Validate the configuration and exit |

`show` prints a definition you can redirect to a file: built-ins get a
placeholder id (`my-clean`) with the Handy attribution, configured ones
keep their real id and note the file they were loaded from. The output of
`show builtin:clean > my.yaml` is a valid definition file once the id is
chosen.

### apply

```sh
pumice format apply --formatter clean < request.json
pumice format apply < request.json          # uses the configured `default`
```

Reads one JSON request from the file given with `--request` (or stdin, the
default), bounded to 1 MiB, and writes the transformed request JSON to
stdout. Without `--json`, failed runs write nothing to stdout (the error goes
to stderr); with `--json`, failures print the error envelope to stdout
instead.

**Shape.** The request must be a JSON object with a `messages` array, and
every message must be a JSON object with a string `role` (positional arrays
and other shapes are refused). A request that repeats a top-level member is
rejected — stricter than the relay, which accepts repeated unknown keys:
the formatter re-emits the request, so it never silently picks one of two
values for the same key.

**Conflict policy.** If any `system` or `developer` message anywhere in the
conversation — not just at the start — carries non-empty content, `apply`
refuses with the error `formatter_instruction_conflict`: the client's own
instructions are never silently replaced or overridden. The check is
conservative: any non-empty content array conflicts, even one holding only
empty text parts, and so does any other value (object, boolean, number).
Absent, null, empty-string and whitespace-only content do not conflict. With `--on-conflict append` the existing instructions are kept
and the formatter's message is inserted right after the leading
`system`/`developer` messages, before the user content.

## Exit codes

- `0`: success (including `--help`/`--version`)
- `2`: invalid arguments, configuration or input — including an unknown
  formatter id and an instruction conflict
- `1`: operational failure, e.g. stdout cannot be written

## Privacy

The formatter handles dictated text, so it never logs it: request content
appears nowhere in error messages, and the library's `Debug` implementations
report lengths and shapes only. Definition and config errors name files,
ids and `file:line:column` positions.
