"""Verify the pinned model, run from an empty directory, and report timings."""
import ast
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile

MODEL_SHA256 = "325b9c7997cd1eff81ef709d55766565e71be696130cc3a3d444713798706834"
binary, model, audio = (Path(arg).resolve(strict=True) for arg in sys.argv[1:])
if hashlib.sha256(model.read_bytes()).hexdigest() != MODEL_SHA256:
    sys.exit("FAIL: model SHA256 mismatch")
with tempfile.TemporaryDirectory(prefix="stt-smoke-") as cwd:
    result = subprocess.run([str(binary), str(model), str(audio)], cwd=cwd,
                            capture_output=True, text=True, encoding="utf-8", timeout=180)
print(result.stderr, file=sys.stderr, end="")
print(result.stdout, end="")
result.check_returncode()
fields = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
if fields.get("language") != "pt" or not ast.literal_eval(fields.get("text", '""')).strip():
    sys.exit("FAIL: expected non-empty Portuguese transcription")
report = f"### {binary.parent.parent.name}\n\nBinary size: {binary.stat().st_size:,} bytes\n\n"
for key in ("audio_seconds", "threads", "model_load_seconds", "session_init_seconds", "transcription_seconds", "language"):
    report += f"- {key}: {fields[key]}\n"
print(report)
if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
    with open(summary, "a", encoding="utf-8") as file:
        file.write(report)
