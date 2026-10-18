---
title: P15.3: browser test page on pumice serve
state: In progress
ticket: —
branch: p15.3-test-page
mr: —
start: 2026-10-18
end: —
---

`pumice serve` serves an embedded test page at `/` (`p15-serve-transcriptions/D8`): it lists the published models from `/v1/models`, records or uploads audio, calls `/v1/audio/transcriptions` on the same origin and shows the final text, the raw transcript, the post-processing status and timings. Strict content security policy, no external assets, the existing Host/Origin guard.
