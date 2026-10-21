#!/usr/bin/env bash
# Benchmark pumice-transcribe CPU builds (portable baseline or cpu-variants).
#
# Runs `pumice-transcribe file --json` on the reference sample with a sweep of
# thread counts and prints one TSV row per run: thread count, run number,
# model_load_ms, transcription_ms and whether the transcript exactly matches
# the expected Turbo text. Timings come from the CLI's own JSON envelope, so
# the numbers are reproducible across builds. The transcript itself is never
# printed.
#
# Defaults match the P7 speed target (p7-transcribe-cpu-variants/D1):
# Whisper Large v3 Turbo Q8_0 on the 14.19 s Portuguese sample.
#
# Usage:
#   crates/transcription/scripts/bench.sh --config PATH [--bin PATH] [--model PATH]
#       [--sample PATH] [--runs N] [--threads "8 12 16 20"] [--cold]
#
# --cold drops the page caches before each run when permitted (needs write
# access to /proc/sys/vm/drop_caches); without it, all runs are warm-cache.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

config=""
bin="$repo_root/target/release/pumice-transcribe"
model="${HOME}/pumice-models/whisper-large-v3-turbo-Q8_0.gguf"
sample="$repo_root/crates/transcription/tests/fixtures/sample-pt.wav"
runs=3
threads_list="8 12 16 20"
cold=0

while [ $# -gt 0 ]; do
    case "$1" in
        --config) config="$2"; shift 2 ;;
        --bin) bin="$2"; shift 2 ;;
        --model) model="$2"; shift 2 ;;
        --sample) sample="$2"; shift 2 ;;
        --runs) runs="$2"; shift 2 ;;
        --threads) threads_list="$2"; shift 2 ;;
        --cold) cold=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ -z "$config" ]; then
    echo "--config PATH is required; copy transcribe.example.yml and fill in every setting" >&2
    exit 2
fi

for path in "$config" "$bin" "$model" "$sample"; do
    if [ ! -f "$path" ]; then
        echo "missing file: $path" >&2
        exit 2
    fi
done

# Expected Turbo transcript for the reference sample (fixtures are not
# dictated text; comparing here avoids printing any transcript at all).
read -r -d '' expected <<'EOF' || true
Esse aqui é um exemplo de texto em português. Então, eu estou dizendo que agora são meia-noite e 36, não está chovendo, está frio e não tem vento.
EOF
expected="$(printf '%s' "$expected" | sed -e 's/[[:space:]]*$//')"

drop_caches() {
    if [ "$cold" -ne 1 ]; then
        return 0
    fi
    if [ -w /proc/sys/vm/drop_caches ]; then
        sync
        echo 3 > /proc/sys/vm/drop_caches
    else
        echo "warning: cannot drop caches (no write access); running warm" >&2
        cold=0
    fi
}

echo "# bin: $bin"
echo "# model: $model"
echo "# sample: $sample"
echo "# cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //') ($(nproc) threads)"
echo "# date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo -e "threads\trun\tmodel_load_ms\ttranscription_ms\ttext_match"

for threads in $threads_list; do
    for run in $(seq 1 "$runs"); do
        drop_caches
        out="$(timeout 300 "$bin" --config "$config" file "$sample" --model-file "$model" \
            --threads "$threads" --json)"
        THREADS="$threads" RUN="$run" EXPECTED="$expected" OUT="$out" python3 - <<'PY'
import json, os, sys

data = json.loads(os.environ["OUT"])
if not data.get("ok"):
    print(f"error: {data}", file=sys.stderr)
    sys.exit(1)
result = data["result"]
text_match = "yes" if result["text"].strip() == os.environ["EXPECTED"] else "no"
print(
    f"{os.environ['THREADS']}\t{os.environ['RUN']}\t"
    f"{result['model_load_ms']}\t{result['transcription_ms']}\t{text_match}"
)
PY
    done
done
