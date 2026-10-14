# Decisions — p5-transcribe-codecs

### D1 · Active · Accept truncated MP3 files that declare no frame count
- date: 2026-10-14
- decided by: brasillero
- origin: conversation
- area: transcription
- revisit if: cut-off MP3 recordings turn out to reach Pumice in practice

**Decision.** A plain MP3 without a Xing/Info or VBRI header is accepted even though a truncated copy cannot be detected; it decodes as a shorter recording. The limit is documented in `crates/transcription/README.md`. MP3 files that declare a frame count, WAV and FLAC are still checked against their declared length.

**Why.** Most encoders write the frame-count header, and refusing headerless MP3s would turn away valid files.

**Rejected.** Rejecting every MP3 that declares no frame count.
