---
title: P9: model catalog and downloader for transcription
state: Done
ticket: —
branch: p9-transcribe-downloader
mr: —
start: 2026-10-15
end: 2026-10-15
---

`pumice-transcribe models catalog|download|verify`: a pinned catalog (Whisper tiny and Large v3 Turbo, Q8_0), explicit downloads into a private per-user models directory with exact size and SHA-256 checks, a single per-download deadline, atomic install re-verified after the rename (D4), JSONL progress and state events for a UI, and `catalog:` model entries in `transcribe.yml`. Nothing is downloaded implicitly. Implemented by Kimi k2.8-code; Codex review rounds 1–3 and a confirming round; Windows behaviour is checked by CI only.
