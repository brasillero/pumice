# Opus fixtures — provenance

All files in this directory except the ones listed here are pre-existing
fixtures (see the git history). The files below were generated on
2026-10-15 for the Opus support task (`p8-transcribe-opus`) and are inputs
to `pumice-transcription` tests. No test or CI step runs ffmpeg; it was used
only to produce these files.

## Source

All ffmpeg-generated files encode the same source:

| File | SHA-256 |
| --- | --- |
| `sample-pt.wav` (existing repo fixture, mono 16 kHz, 14.190 s, RMS 0.0094633070) | `3c217cd883d5560688e10d82b91cdda24892e7734f4b9f97e03de39d97a03d24` |

## Tool

`ffmpeg-7.0.2-amd64-static` (johnvansickle.com static build), used for
generation only, run as `ffmpeg -i sample-pt.wav <args> <output>` from the
research scratch directory (`~/scratch/opus-research`).

| File | ffmpeg arguments | SHA-256 |
| --- | --- | --- |
| `sample-pt-48k.opus` | `-ac 1 -ar 48000 -c:a libopus -b:a 64k` | `de20a46371df7f501015aefc060a04a42adcbabb045cde9c19e72122dfdac982` |
| `sample-pt.webm` | `-ac 1 -ar 48000 -c:a libopus -b:a 64k` | `daf4e6df70134717b318653c4a1443f3f3e62e9ab5b5a5f93450722d94eb8b61` |
| `sample-pt-live.webm` | `-ac 1 -ar 48000 -c:a libopus -b:a 64k -live 1 -cluster_time_limit 1000` | `d79e1b56bbb0db9a75ef377a7d32346e9762def8be9e64602aaa4d3d41c67d09` |
| `sample-pt-stereo.webm` | `-af pan=stereo\|c0=c0\|c1=c0 -ar 48000 -c:a libopus -b:a 96k -live 1 -cluster_time_limit 1000` | `ca290f8df66cb2490aa9967b5f1231135d38941c7c5126c59fa8daa4d0ededf7` |

Decoded expectations at 16 kHz (measured with the reference libopus during
research): the ffmpeg mono fixtures decode to exactly 227,040 samples
(RMS 0.0094542735; stereo duplicate 0.0094756185); `sample-pt.webm` carries
end DiscardPadding (3.5 ms) and CodecDelay 6.5 ms.

## Duration boundary inputs

Round 1 review reproductions of the 120 s output budget, generated on
2026-10-15 with the same ffmpeg static build: 48 kHz mono digital silence
encoded with libopus. `silence120.opus` keeps exactly 120.0 s after its
pre-skip and end trim (final granule minus pre-skip = 5,760,000 samples);
`silence121.webm` carries 121 s and must fail as too long.

| File | ffmpeg arguments | SHA-256 |
| --- | --- | --- |
| `silence120.opus` | `-f lavfi -i anullsrc=r=48000:cl=mono -t 120 -c:a libopus -b:a 8k` | `3616ccb6d1eacbc1d2e18868112dd7b42546d36d8175c81ce514e119eccfaa24` |
| `silence121.webm` | 121 s of the same silence, libopus in WebM (exact flags not recorded by the review harness) | `57f901245c63115ab7637372c09c834ec99363fa83b9f91e37da8b4a8bcacb88` |

## Browser recording

`chrome-mediarecorder.webm`
(SHA-256 `7374ddaeaf83748456586428e13e8c76ed8a60184a1514e89be71057f409e47e`)
is a real Chrome 154.0.0.0 (Linux x86_64) MediaRecorder output produced
during research: WebAudio `decodeAudioData(sample-pt.wav)` →
`MediaStreamAudioDestinationNode` → `MediaRecorder` (default options),
`start(1000)`, stopped on source end. MIME `audio/webm;codecs=opus`, 14
unknown-size clusters, no Duration/Cues, OpusHead pre-skip 0, no
CodecDelay/DiscardPadding. MediaRecorder timing and bytes are not
deterministic; do not regenerate this file expecting identical output.
Decoded expectation at 16 kHz: exactly 226,560 samples (RMS 0.0094989115).
