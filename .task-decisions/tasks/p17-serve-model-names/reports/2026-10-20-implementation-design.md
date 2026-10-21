# P17 implementation design (Codex, 2026-10-20, read-only at 5dfab4a)

Proposals, not owner decisions; the owner decisions are D1–D7.

## Order

1. Explicit module configuration (D3) as its own prerequisite task and PR.
2. Folder-based transcription inventory (D4, D7).
3. Served model registry, expansion and relay-off execution (D1, D2).

## Proposed `serve:` section (all keys required)

```yaml
serve:
  enabled: true
  port: 17570
  transcription_model: whisper-large-v3-turbo-q8_0
  startup_timeout_ms: 60000
  allowed_origins:
    - https://server-1.tail59f332.ts.net:17570
  budgets_ms:
    total: 180000
    transcription: 120000
    formatter: 1000
    relay: 60000
  relay:
    enabled: true
    default_provider: codex
    default_preset: clean
    presets: [clean, clean-en]
    expand_providers: false
    expand_presets: true
```

- The starting YAML writes both expansion switches as `false`: raw plus the default combination.
- Expansion order: providers as in `relay.yml`, presets as in `serve.relay.presets`; both switches give the Cartesian product. `default_preset` must be in `presets`. `passthrough` and `inspect` are not expandable.
- `presets` are formatter references (rules stay in `formatter.yml`); `builtin:clean` is allowed, giving `model:codex:builtin:clean`.
- `serve.transcription_flows` is removed and rejected with a migration message. Dictation `flows:` keep their ids; only `turbo` becomes the canonical id.
- `serve.budgets_ms.relay` applies to every generated combination; provider settings stay in `relay.yml`.

## Proposed `transcribe.yml`

```yaml
schema_version: 1
default_model: whisper-large-v3-turbo-q8_0   # selection, not an alias; null allowed
language: pt          # null = detect
threads: auto
backend: cpu
server:
  port: 17568
models_dir: null      # null = platform folder (D7); a string overrides, relative to the file
```

- `models:` is removed; discovery builds the inventory.
- Discovery: immediate children only, regular `.gguf` files; ignore directories, partial downloads, `SHA256SUMS`, other extensions.
- Catalog files are recognised by size and SHA-256 (hash only size matches), independent of the file name; others keep the exact file stem.
- Reject empty, non-UTF-8, control-character and over-512-byte ids, and duplicate ids (naming both files). File symlinks allowed (target resolved), directory symlinks not followed, broken `.gguf` symlinks reported.
- Snapshot at process start; no watcher. A restarted child must reload the same id or fail.
- An empty folder or missing default must not block `models catalog` or downloads; existence is checked before inference starts.
- `file --model-file PATH` stays for one-off files.
- Hashing may exceed today's fixed 5 s preflight: use the explicit startup budget.

## D3 inventory (built-in defaults today)

- Orchestrator: `flows` empty; `serve` absent; `modules.formatter` absent; flow `enabled` false; budgets 180000 / 120000 / 1000 / 60000 ms; serve `enabled` false, port 17570, startup 60000, flows and origins empty.
- Relay: missing file = built-in config; port 7567; total timeout 30 s; `max_parallel` 4; providers empty; debug log off, path `pumice-debug.jsonl`; provider binaries `claude`, `codex`, `kimi`, `kiro-cli`; provider timeout 30 s.
- Transcription: missing file = empty; models empty; default unset; language detect; threads auto (cap 32); backend cpu; models folder = platform folder. Server port has no default.
- Formatter: missing file = empty; `formatters` empty; `default` unset.
- Needs presence-aware parsing (omitted vs explicit `null`), with missing keys reported at the enclosing mapping's line.
- AGENTS.md sentence proposed: "Settings must be explicit in YAML; omitted settings are startup errors with file and line. Defaults exist only in the initial configuration."

## Registry and API

- New `serve/models.rs`: an immutable registry of `{public_id, transcription_model, provider?, preset?, budgets}`, validated before listening. Lookup is exact and case-sensitive, never by splitting ids.
- `GET /v1/models/{id}`: percent-decode once, validate UTF-8, keep a literal `+`; malformed gives 400, unknown gives 404 `model_not_found`.
- Raw execution returns after transcription and builds no relay pipeline. The relay pipeline is built only for AI plans.
- Extension fields for generated ids: `pumice.flow = null`, `pumice.model` = the requested id, `pumice.transcription_model`, `post_processing.preset` / `relay_route` (`passthrough` for raw). P15 D4/D9 fallbacks are kept.

## Migration (owner's server-1)

1. Back up the four YAMLs and the binaries.
2. Stop `pumice-serve`, verify `SHA256SUMS`, then move both `.gguf` files and `SHA256SUMS` to `~/.local/share/pumice/models` without overwriting; verify again.
3. Rewrite `transcribe.yml` and `serve:`; change dictation flows from `turbo` to the canonical id.
4. Complete `relay.yml` for D3: `port: 7567`, `max_parallel: 4`, explicit binaries, `debug_log.path`.
5. Validate with the candidate binaries, deploy matching `pumice` and `pumice-transcribe`, and restart `pumice-serve`; restart `pumice.service` (7567) only if the relay schema changed.
6. Check both ports. Roll back YAMLs and binaries together.

## Open choices for the owner

- Catalog identity: SHA-256 after size filter (recommended), or exact catalog file name.
- Relay off: disables AI processing in all of `pumice serve` (AI dictations report `relay_disabled`, raw stays; recommended), or only hides formatted transcription ids.
- D3: separate prerequisite PR (recommended), or only the new keys now.
