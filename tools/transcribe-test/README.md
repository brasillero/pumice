# Transcription browser test

A standalone development client for Pumice's OpenAI-compatible transcription API.
No build step, packages, CDN, or external assets. The page uses only its configured
endpoint; the helper uses Python 3's standard library.

From the repository root, with a configured model (or pass `--config` and `--model`):

```sh
pumice-transcribe serve --port 17568
```

In another terminal:

```sh
python3 -I tools/transcribe-test/serve.py
```

Open <http://127.0.0.1:17569>. The page defaults to `/v1`; the helper proxies that
path to `http://127.0.0.1:17568/v1`. Refresh models, select the served model, then
click **Record**, speak, and click **Stop & transcribe**. At 120 seconds it stops
and sends automatically. You can also select an existing file and click
**Transcribe selected file**. An empty language enables server configuration or
automatic detection. Prompt text is sent unchanged. The optional API key is sent
only when filled, as `Authorization: Bearer …`; it is never saved.

The transcript, HTTP status, round-trip time (through receipt of the response
body), raw response, and OpenAI error details appear below. HTTP 429 means the
server is busy; retry manually after its current transcription finishes. The ten
latest results stay in page memory until reload. Only the endpoint base URL is
remembered in localStorage. Transcripts and audio are not persisted by this tool.

Custom ports and upstream:

```sh
python3 -I tools/transcribe-test/serve.py --port 17579 --upstream http://127.0.0.1:17578
```

The upstream must be plain HTTP on `127.0.0.1`, without a path. The helper always
binds `127.0.0.1`. It streams bodies in 64 KiB chunks, caps total request bodies at
26,000,000 bytes, passes multipart boundaries, authorization, response bytes and
HTTP statuses through, and strips hop-by-hop headers. It does not log requests or
responses. It requires Content-Length (browser fetch supplies it); chunked uploads
get 411. Socket timeouts are 15 seconds while reading uploads and 135 seconds for
upstream operations. It is a development helper, without authentication or
production hardening. Ctrl-C stops it.

## Direct mode

Open `tools/transcribe-test/index.html` directly in a browser and use
`http://127.0.0.1:17568/v1` as the base URL. Local file pages default to this URL;
HTTP/HTTPS pages default to `/v1`. A saved URL takes precedence in either mode.
When using another static web server, set the base URL explicitly.

A direct request from a different origin (including a local file) requires the
transcription server to permit CORS. Pumice may not send CORS headers. An API key
also causes a CORS preflight. Use the helper when CORS blocks the request; no
server changes are needed. HTTPS pages cannot normally fetch insecure remote HTTP
URLs. For a remote browser, `127.0.0.1` means that browser's machine, so use the
same-origin `/v1` proxy instead.

## Optional access from another machine

Microphone capture requires a secure context: loopback HTTP works locally;
remote access needs HTTPS. If the owner chooses to expose this development helper
through their Tailscale setup, the proposed command is:

```sh
tailscale serve --bg --https=17569 http://127.0.0.1:17569
```

Open the HTTPS URL reported by Tailscale on the other machine and keep the base
URL `/v1`. This exposes both the page and proxy to clients permitted by the
owner's Tailscale policy. Running/configuring Tailscale is an owner action; this
tool does not configure tunnels or change any existing services. The API key
field only mimics SDK clients; it adds no authentication to this helper.

## Limits

- Microphone capture uses Web Audio's ScriptProcessor compatibility API, which is
  deprecated but keeps the page in one file without a separate worklet. It mixes
  to mono and writes 16-bit PCM WAV at the AudioContext's native sample rate
  (typically 44.1 or 48 kHz). The transcription server resamples to 16 kHz.
- Browser microphone permissions, device availability, and Web Audio support are
  required. Keep the tab active; suspended/background tabs can lose audio or
  delay automatic stopping. Both the sample budget and timer cap recording at
  120 seconds; the timer can be delayed by browser suspension.
- Audio must be at most 25,000,000 bytes and 120 decoded seconds. Unusual high
  native sample rates can reach the byte limit before 120 seconds. Upload
  duration and actual codecs are validated by the server.
- Supported uploads: WAV PCM/float, MP3, FLAC, Ogg/Vorbis. Opus/WebM and AAC/MP4
  are unsupported. This page deliberately avoids MediaRecorder's lossy formats.
- The page requests only model discovery and transcription. No health polling,
  retries, cancellation UI, streaming transcription, temperature control, or
  automatic model changes. Fetch times out after 135 seconds; server-side
  cancellation and deadlines remain the server's responsibility.
- The page renders response text as plain text. Raw responses and history can
  contain sensitive transcripts: close/reload the page to clear them.
