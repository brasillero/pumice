# Decisions — p17-serve-model-names

### D1 · Active · Served model ids are `model`, or `model:relay:preset` when the relay is on
- date: 2026-10-20
- decided by: brasillero
- origin: conversation
- area: api
- revisit if: —

**Decision.** `GET /v1/models` of `pumice serve` lists the transcription model by its real name (for example `whisper-large-v3-turbo-q8_0`), which always returns the raw transcript. With the relay on it also lists `model:relay:preset` ids (for example `whisper-large-v3-turbo-q8_0:codex:clean`): the relay provider that formats and the formatter preset it applies. With the relay off only the plain model is listed. The separator is `:`.

**Why.** The owner wants `pumice serve` to read like any OpenAI-compatible transcription server, with the choices visible in the id, instead of Pumice-only flow names (`raw`, `clean`, `clean-en`). The plain id keeps raw text selectable from generic clients.

**Rejected.** Middle dot `·` (needs URL encoding, hard to type); `--` (less conventional than Ollama-style `:`); plain id applying the default preset (raw would no longer be selectable, and a translating default would surprise clients); flow names as ids (`p15-serve-transcriptions/D5`).

**Consequences.** Replaces `p15-serve-transcriptions/D5` for `pumice serve`; dictation flows stay. The provider for each formatted id comes from the id itself.

### D2 · Active · Expanding relays and presets is configured, with the defaults written in YAML
- date: 2026-10-20
- decided by: brasillero
- origin: conversation
- area: config
- revisit if: —

**Decision.** Without expansion, the relay-on entry uses the default relay provider and default preset written in the YAML (the owner's: `codex` and `clean`). One option expands to every enabled relay provider, another to every listed preset; with both, every combination is listed.

**Why.** The owner wants a short list by default and the option to offer every provider and preset to a client such as T3 Code.

**Rejected.** Publishing every combination always: long lists; per-preset providers fixed in config: the id already names the provider.

### D3 · Active · Settings have no hidden defaults
- date: 2026-10-20
- decided by: brasillero
- origin: conversation
- area: config
- revisit if: —

**Decision.** Every setting must be written in the YAML; omitting one is a startup error with the file and line, never a silent default. "Default" means the starting YAML that Pumice writes. Settings that apply to the whole relay, such as a timeout, may be one global value instead of one per provider.

**Why.** The owner wants nothing to happen because something was left out.

**Rejected.** Built-in defaults for omitted keys (today's rule for ports and other global settings).

**Consequences.** The owner authorized changing the AGENTS.md sentence "Global settings such as ports have built-in defaults" in the implementing pull request. It also answers `p16-provider-effort#1`'s default question: the effort must be written.

### D4 · Active · Transcription models are the files in a models folder
- date: 2026-10-20
- decided by: brasillero
- origin: conversation
- area: transcription
- revisit if: —

**Decision.** Putting a model file in the configured models folder makes it available; a file from Pumice's catalog takes its catalog id (`whisper-large-v3-turbo-q8_0`), any other file its file name without `.gguf`. `pumice serve` still serves one model at a time.

**Why.** The owner wants the real model names instead of hand-written aliases (`turbo`, `tiny`); the folder is the explicit opt-in.

**Rejected.** Hand-written ids in `transcribe.yml` (names drift from the real model); names derived from catalog discovery without files.

**Consequences.** Which folder (Pumice's default `~/.local/share/pumice/models` or another) is open in #2.

### D5 · Active · Pumice's response extensions stay and generic clients ignore them
- date: 2026-10-20
- decided by: brasillero
- origin: conversation
- area: api
- revisit if: a client rejects the extra fields

**Decision.** The `pumice` object and the `Pumice-Postprocessing-Status` header stay in `pumice serve` answers (`p15-serve-transcriptions/D6`). Generic clients, including the T3 demo, use only `text`.

**Why.** OpenAI clients ignore unknown fields, and Pumice's own test page uses them.

**Rejected.** Removing them or making them opt-in.

### D6 · Active · The T3 voice demo is a generic OpenAI-compatible transcription client
- date: 2026-10-20
- decided by: brasillero
- origin: conversation
- area: api
- revisit if: —

**Decision.** The demo settings have the server URL, an optional API key (stored on the T3 server, sent as `Authorization: Bearer` only when set, never returned to the browser), a searchable model list filled from `/v1/models` with a "Use '<typed id>'" choice for ids not in the list, and the language. Nothing in it is Pumice-specific; the post-processing warning goes.

**Why.** The OpenAI standard has no field marking transcription models (reports research of 2026-10-20 in conversation), and the owner wants to pitch a feature that works with any compatible server, Pumice included.

**Rejected.** Filtering models by name (`whisper`, `transcribe`): hides Pumice ids and other models; a plain select: unusable when a server has no model list.

### D7 · Active · Transcription models live in Pumice's default models folder
- date: 2026-10-20
- decided by: brasillero
- origin: D4
- area: transcription
- revisit if: —

**Decision.** The models folder for D4 is Pumice's default per-user folder (`~/.local/share/pumice/models` on Linux; `models_dir` in `transcribe.yml` can still name another). The owner's two files move there from `~/pumice-models` when this ships.

**Why.** It is where catalog downloads already go, follows the platform convention for app data, and needs no setting.

**Rejected.** A folder next to the config files: model files are large (about 900 MB for turbo) and config folders are kept small.

### D8 · Active · Catalog models are recognised by size and SHA-256
- date: 2026-10-20
- decided by: brasillero
- origin: reports/2026-10-20-implementation-design.md
- area: transcription
- revisit if: —

**Decision.** A file in the models folder takes its catalog id when its size and SHA-256 match a catalog entry, whatever its file name; other `.gguf` files keep their file name without the extension.

**Why.** A renamed catalog file is still recognised, and a file is never given a catalog name from its name alone.

**Rejected.** Matching the exact catalog file name: breaks on renames and trusts the name.

### D9 · Active · With the relay off, `pumice serve` runs no AI processing
- date: 2026-10-20
- decided by: brasillero
- origin: reports/2026-10-20-implementation-design.md
- area: api
- revisit if: —

**Decision.** When the serve relay setting is off, `pumice serve` builds no relay or formatter runtime: `GET /v1/models` lists only the plain model ids, AI dictation flows report that the relay is disabled, and raw dictations keep working. `pumice run` is not affected.

**Why.** "Relay off" should mean no AI calls at all, and choices that cannot run must not appear in the model list.

**Rejected.** Only hiding the formatted ids while AI dictations still run.

### D10 · Active · Explicit settings ship as their own pull request before the model changes
- date: 2026-10-20
- decided by: brasillero
- origin: reports/2026-10-20-implementation-design.md
- area: process
- revisit if: —

**Decision.** D3 is implemented for all four module configs in a separate first pull request; the models folder (D4, D7, D8) and the served model ids (D1, D2, D9) follow in their own pull requests.

**Why.** Small pull requests, and the later changes can then require their new keys without exceptions.

**Rejected.** Requiring only the new `serve` keys now and leaving the rest for later.
