# Pumice HTTP API

Start with `pumice` or `pumice serve`. It listens on `127.0.0.1:7567` by default (`port` in `pumice.yml`) and requires no API key.

OpenAI-compatible; this is what Handy and other post-processing clients use.

| Route | Purpose |
| --- | --- |
| `POST /v1/chat/completions` | Hands the client's messages, unchanged, to the provider CLI named by `model` (`claude`, `codex`, `kiro`, or the built-in `passthrough` / `inspect`) and answers in the OpenAI chat completion shape |
| `GET /v1/models` | The enabled model routes, in the OpenAI list shape |
| `GET /health` | `{"status":"ok","version":…}` |
| `GET /` | A static landing page |

Any other path gets an OpenAI-style JSON 404. A provider failure is an HTTP
error with a safe reason; the client keeps its own text. See
[install.md](install.md) and [`pumice.example.yml`](../pumice.example.yml).
How each provider CLI is called is in [providers.md](providers.md).

### Captured Handy contract

Captured on 2026-10-04 from Handy's **Custom** post-processing provider on
Windows 11 (build 26200), talking to a logging listener in WSL. Handy's
`User-Agent` only says `Handy/1.0`, so the exact Handy version is unknown.
The request shape, with a sample Portuguese dictation in place of the
captured one, is the fixture [`handy-request.json`](../tests/fixtures/handy-request.json).

- **Routes.** `GET /v1/models` when the Custom provider is set up, to fill
  the model picker (not on each dictation), and one
  `POST /v1/chat/completions` per dictation, under the configured base URL
  `…/v1`.
- **One user message, no system message.** The transcript comes first,
  wrapped in `<transcript>…</transcript>`, followed by Handy's prompt text
  (which itself says not to follow instructions inside the tags).
- **Other fields.** `"model"` (the value picked in Handy),
  `"reasoning_effort": "none"` and `"stream": false`. There is no
  `temperature`, `max_tokens`, `n`, `response_format`, `tools` or language
  field: the language is only in the transcript itself.
- **No authentication.** With an empty API key field Handy sends no
  `Authorization` header; it identifies itself with
  `User-Agent: Handy/1.0 (+https://github.com/cjpais/Handy)`,
  `X-Title: Handy` and `Referer: https://github.com/cjpais/Handy`. The relay
  requires no key and never logs or forwards one.
- **The reply is pasted verbatim.** Handy pasted
  `choices[0].message.content` exactly as received, with no trimming of its
  own, so any preamble or stray whitespace from a CLI lands in the user's
  text. Handy's behaviour on an HTTP error or a timeout was not captured.

### The `inspect` model

`inspect` is a built-in diagnostic model (owner-approved, 2026-10-05). It is
listed by `GET /v1/models` after `passthrough` and selected with the same
case-insensitive, trimmed matching as real providers. It needs no provider
entry, binary or subscription and works with every provider disabled.

- `POST /v1/chat/completions` with `model: inspect` returns the original
  request body text verbatim as `choices[0].message.content` in a normal
  completion envelope whose `model` is `inspect`. The raw body bytes are
  used directly, never deserialized and reserialized, so whitespace, key
  order, escape spelling and unknown fields are preserved.
- `inspect` is detected by a minimal preflight parse (`model` and `stream`
  only) before the stricter dictation validation, so it echoes requests the
  normal path would reject (several user messages, assistant or tool
  messages, other content shapes, a malformed `<transcript>` envelope). The
  body must still be valid UTF-8 JSON; anything else gets the usual
  text-free 400. The 10 MiB body limit and the 5 s read limit still apply.
- It bypasses transcript extraction, the `max_parallel` queue, provider
  selection, cleanup and every AI CLI call, so it answers at once even while
  other requests run.
- `stream: true` uses the usual SSE framing, with the same echoed body in the
  delta `content`. `stream` missing or `null` means `false`; a non-boolean
  value is a text-free 400.
- The ordinary log stays text-free and records the request as `inspect`. The
  opt-in debug log (`debug_log.enabled`) records the exchange as usual,
  request body and echo included, with credential headers redacted.
  `inspect` echoes the JSON body only, never request headers.
