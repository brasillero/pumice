"""Tests for the pumice npm launcher (scripts/npm/bin/pumice.js).

Builds a fake installed-package layout in a temporary directory — front
package plus a fake native binary — and drives the launcher through Node
with a real process in front of it. Covers argv forwarding (including
spaces and shell metacharacters), stdin/stdout/stderr pass-through, exit
code and signal propagation, and the clear error messages for a missing
native dependency, a version mismatch, an unsupported platform and a
missing binary. Also pins down that the launcher never resolves the
native package from the caller's working directory. No network, no real
pumice binary.
"""

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
LAUNCHER_SOURCE = REPO_ROOT / "scripts" / "npm" / "bin" / "pumice.js"
NODE = shutil.which(os.environ.get("PUMICE_NODE", "node"))

FRONT_VERSION = "9.9.9"
NATIVE_NAME = "@brasillero/pumice-linux-x64"

# Echoes every argument, the working directory and stdin; writes one line to
# stderr; exits with FAKE_PUMICE_EXIT (default 0). Plain /bin/sh, no bashisms.
FAKE_BINARY = """#!/bin/sh
for arg in "$@"; do printf 'arg=<%s>\\n' "$arg"; done
printf 'cwd=<%s>\\n' "$PWD"
printf 'stdin=<%s>\\n' "$(cat)"
echo 'stderr-line' >&2
exit "${FAKE_PUMICE_EXIT:-0}"
"""

# Writes a marker file when terminated, so the test can prove the signal
# reached the real process behind the launcher. `wait` is interruptible by
# signals, unlike a foreground `sleep`, so the trap runs immediately.
SIGNAL_BINARY = """#!/bin/sh
trap 'echo termed >"$FAKE_PUMICE_MARKER"; exit 143' TERM INT
echo ready
sleep 30 &
wait $!
"""


@unittest.skipIf(NODE is None, "node is required for launcher tests")
class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.front = self.root / "front"
        self.native = (
            self.front / "node_modules" / "@brasillero" / "pumice-linux-x64"
        )
        self._write_layout()

    def _write_layout(self, native_version=FRONT_VERSION, native_name=NATIVE_NAME):
        (self.front / "bin").mkdir(parents=True, exist_ok=True)
        shutil.copy(LAUNCHER_SOURCE, self.front / "bin" / "pumice.js")
        (self.front / "package.json").write_text(
            json.dumps(
                {
                    "name": "@brasillero/pumice",
                    "version": FRONT_VERSION,
                    "optionalDependencies": {native_name: FRONT_VERSION},
                }
            )
        )
        (self.native / "bin").mkdir(parents=True, exist_ok=True)
        (self.native / "package.json").write_text(
            json.dumps({"name": NATIVE_NAME, "version": native_version})
        )
        (self.native / "bin" / "pumice").write_text(FAKE_BINARY)
        (self.native / "bin" / "pumice").chmod(0o755)

    def run_launcher(self, args, *, env=None, cwd=None, **popen):
        full_env = dict(os.environ)
        full_env.update(env or {})
        popen.setdefault("timeout", 30)
        return subprocess.run(
            [NODE, str(self.front / "bin" / "pumice.js"), *args],
            input="",
            capture_output=True,
            text=True,
            env=full_env,
            cwd=cwd or self.root,
            **popen,
        )

    def test_argv_stdin_stderr_and_exit_code_forwarded(self):
        args = ["plain", "with space", "semi;colon|pipe", "$HOME", "glob*"]
        env = {"FAKE_PUMICE_EXIT": "7"}
        result = subprocess.run(
            [NODE, str(self.front / "bin" / "pumice.js"), *args],
            input="hello stdin",
            capture_output=True,
            text=True,
            env={**os.environ, **env},
            cwd=self.root,
            timeout=30,
        )
        self.assertEqual(result.returncode, 7, result.stderr)
        for arg in args:
            self.assertIn(f"arg=<{arg}>", result.stdout)
        self.assertIn(f"cwd=<{self.root}>", result.stdout)
        self.assertIn("stdin=<hello stdin>", result.stdout)
        self.assertIn("stderr-line", result.stderr)

    def test_exit_code_zero(self):
        result = self.run_launcher(["--version"])
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_sigterm_reaches_child_and_exit_propagates(self):
        marker = self.root / "marker"
        (self.native / "bin" / "pumice").write_text(SIGNAL_BINARY)
        (self.native / "bin" / "pumice").chmod(0o755)
        process = subprocess.Popen(
            [NODE, str(self.front / "bin" / "pumice.js")],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env={**os.environ, "FAKE_PUMICE_MARKER": str(marker)},
            cwd=self.root,
        )
        try:
            self.assertEqual(process.stdout.readline().strip(), "ready")
            process.send_signal(signal.SIGTERM)
            process.wait(timeout=15)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=15)
            process.stdout.close()
            process.stderr.close()
        self.assertTrue(marker.is_file(), "SIGTERM never reached the child")
        self.assertIn(
            process.returncode, (143, -signal.SIGTERM), process.returncode
        )

    def test_missing_native_dependency_fails_clearly(self):
        shutil.rmtree(self.front / "node_modules")
        result = self.run_launcher([])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing native package", result.stderr)
        self.assertIn(NATIVE_NAME, result.stderr)

    def test_caller_cwd_cannot_satisfy_missing_dependency(self):
        # A fake native package planted in the calling directory must NOT
        # rescue an installation whose real dependency is missing.
        shutil.rmtree(self.front / "node_modules")
        cwd = self.root / "elsewhere"
        fake = cwd / "node_modules" / "@brasillero" / "pumice-linux-x64"
        (fake / "bin").mkdir(parents=True)
        (fake / "package.json").write_text(
            json.dumps({"name": NATIVE_NAME, "version": FRONT_VERSION})
        )
        (fake / "bin" / "pumice").write_text(FAKE_BINARY)
        (fake / "bin" / "pumice").chmod(0o755)
        result = self.run_launcher([], cwd=cwd)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing native package", result.stderr)

    def test_version_mismatch_fails_clearly(self):
        self._rewrite_native_version("8.8.8")
        result = self.run_launcher([])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("version mismatch", result.stderr)
        self.assertIn("8.8.8", result.stderr)
        self.assertIn(FRONT_VERSION, result.stderr)

    def test_unsupported_platform_fails_clearly(self):
        self._write_layout(native_name="@brasillero/pumice-fuchsia-arm64")
        # Point the layout at a dependency name that cannot match this host.
        result = self.run_launcher([])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsupported platform", result.stderr)
        self.assertIn("linux-x64", result.stderr)

    def test_missing_binary_fails_clearly(self):
        (self.native / "bin" / "pumice").unlink()
        result = self.run_launcher([])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("native binary missing", result.stderr)

    def _rewrite_native_version(self, version):
        (self.native / "package.json").write_text(
            json.dumps({"name": NATIVE_NAME, "version": version})
        )


if __name__ == "__main__":
    unittest.main()
