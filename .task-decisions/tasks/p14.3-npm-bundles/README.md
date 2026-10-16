---
title: P14.3: npm platform packages carry the transcription folder
state: In progress
ticket: —
branch: p14.3-npm-bundles
mr: —
start: 2026-10-16
end: —
---

Ship Pumice through npm as one CLI installed as a folder (`arch-modules/D16`): each platform package's `bin/` holds `pumice` next to the complete `pumice-transcribe` bundle for that platform (core libraries, CPU modules, GPU module, `bundle.json`), so `npm i -g @brasillero/pumice` gives working transcription with no download at runtime. Release preview builds the core and the transcription bundles at the same commit, packs the packages and installs them on each OS through the local test registry. Publishing stays owner-operated. Design: `reports/2026-10-16-npm-bundle-design.md` (Codex).
