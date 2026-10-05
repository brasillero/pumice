"""Install tests for the pumice npm packages against a loopback registry.

Builds five fake tarballs (a front package with the real launcher plus four
native packages with a fake shell binary), serves them through the
loopback test registry in scripts/npm_registry_fixture.py, and installs the
front package with npm, pnpm and Bun pointed at that registry. Asserts that
only the platform-matching optional dependency is installed, that
``node_modules/.bin/pumice`` runs the fake binary, and that an isolated
global npm prefix install and npx work.

The owner's environment is never touched: every tool runs with explicit
per-manager config/cache locations (empty npm userconfig/globalconfig,
npm_config_cache, XDG_* dirs, PNPM_HOME, a project-local bunfig.toml with
its own cache dir) and temporary project directories. HOME and
USERPROFILE are deliberately left untouched. The fake native binary is a
POSIX shell script, so the execution assertions are skipped on Windows
(the CI smoke job covers Windows with the real binary).

Tools are located via PUMICE_NPM / PUMICE_PNPM / PUMICE_BUN (a command line,
parsed with shlex) or PATH; missing tools skip their tests, so the suite
runs anywhere and CI installs all of them.
"""

import json
import os
import platform
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import io
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import npm_registry_fixture

REPO_ROOT = Path(__file__).resolve().parent.parent
LAUNCHER_SOURCE = REPO_ROOT / "scripts" / "npm" / "bin" / "pumice.js"

FRONT_VERSION = "9.9.9"
PLATFORMS = ("linux-x64", "win32-x64", "darwin-x64", "darwin-arm64")

# Echoes its arguments so tests can prove the .bin entry reached the native
# binary through the launcher. chmod 0755 when packaged.
FAKE_BINARY = """#!/bin/sh
printf 'fake-pumice'
for arg in "$@"; do printf ' <%s>' "$arg"; done
printf '\\n'
"""


def find_tool(env_var: str, binary: str):
    override = os.environ.get(env_var)
    command = shlex.split(override) if override else [binary]
    if override:
        return command
    path = shutil.which(binary)
    return [path] if path else None


NPM = find_tool("PUMICE_NPM", "npm")
PNPM = find_tool("PUMICE_PNPM", "pnpm")
BUN = find_tool("PUMICE_BUN", "bun")
NODE = find_tool("PUMICE_NODE", "node")

IS_WINDOWS = platform.system() == "Windows"


def make_tarball(root: Path, files: dict[str, tuple[bytes, int]]) -> Path:
    manifest = json.loads(files["package/package.json"][0])
    base = manifest["name"]
    base = base[1:].replace("/", "-") if base.startswith("@") else base
    path = root / f"{base}-{manifest['version']}.tgz"
    with tarfile.open(path, "w:gz") as archive:
        for name, (payload, mode) in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            info.mode = mode
            archive.addfile(info, fileobj=io.BytesIO(payload))
    return path


def build_fake_packages(root: Path) -> None:
    """Creates the five fake .tgz files in root."""
    launcher = LAUNCHER_SOURCE.read_bytes()
    license_bytes = (REPO_ROOT / "LICENSE").read_bytes()
    optional = {
        f"@brasillero/pumice-{key}": FRONT_VERSION for key in PLATFORMS
    }
    make_tarball(
        root,
        {
            "package/package.json": (
                json.dumps(
                    {
                        "name": "pumice",
                        "version": FRONT_VERSION,
                        "license": "MIT",
                        "type": "commonjs",
                        "bin": {"pumice": "bin/pumice.js"},
                        "engines": {"node": ">=22"},
                        "optionalDependencies": optional,
                    }
                ).encode(),
                0o644,
            ),
            "package/bin/pumice.js": (launcher, 0o755),
            "package/LICENSE": (license_bytes, 0o644),
        },
    )
    executable = "pumice.exe" if IS_WINDOWS else "pumice"
    for key in PLATFORMS:
        npm_os, _, npm_cpu = key.partition("-")
        make_tarball(
            root,
            {
                "package/package.json": (
                    json.dumps(
                        {
                            "name": f"@brasillero/pumice-{key}",
                            "version": FRONT_VERSION,
                            "license": "MIT",
                            "os": [npm_os],
                            "cpu": [npm_cpu],
                            "files": [f"bin/{executable}"],
                        }
                    ).encode(),
                    0o644,
                ),
                f"package/bin/{executable}": (
                    FAKE_BINARY.encode(),
                    0o755,
                ),
                "package/LICENSE": (license_bytes, 0o644),
            },
        )


class InstallTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.work = Path(cls.tmp.name)
        cls.packages = cls.work / "packages"
        cls.packages.mkdir()
        build_fake_packages(cls.packages)
        cls.registry = npm_registry_fixture.serve(cls.packages)
        cls.addClassCleanup(cls.registry.stop)

    def setUp(self):
        self.project = Path(tempfile.mkdtemp(prefix="pumice-pm-test-"))
        self.addCleanup(shutil.rmtree, self.project, True)
        # Per-manager isolation: empty npm configs and caches inside the
        # temporary project. HOME/USERPROFILE are intentionally NOT
        # overridden.
        (self.project / "npm-userconfig.ini").write_text("")
        (self.project / "npm-globalconfig.ini").write_text("")
        (self.project / "xdg-config").mkdir()
        (self.project / "xdg-cache").mkdir()
        (self.project / "xdg-data").mkdir()
        (self.project / "pnpm-home").mkdir()
        # bun reads bunfig.toml from the cwd and nothing else global here.
        (self.project / "bunfig.toml").write_text(
            "[install.cache]\n"
            f'dir = "{self.project / "bun-cache"}"\n'
        )

    def run_tool(self, command, *args, cwd=None):
        env = dict(os.environ)
        env.update(
            {
                "npm_config_userconfig": str(self.project / "npm-userconfig.ini"),
                "npm_config_globalconfig": str(self.project / "npm-globalconfig.ini"),
                "npm_config_cache": str(self.project / "npm-cache"),
                "XDG_CONFIG_HOME": str(self.project / "xdg-config"),
                "XDG_CACHE_HOME": str(self.project / "xdg-cache"),
                "XDG_DATA_HOME": str(self.project / "xdg-data"),
                "PNPM_HOME": str(self.project / "pnpm-home"),
            }
        )
        result = subprocess.run(
            [*command, *args],
            cwd=cwd or self.project,
            env=env,
            capture_output=True,
            text=True,
            timeout=180,
        )
        return result

    def require_fake_exec(self):
        """The fake native binary is a POSIX shell script."""
        if IS_WINDOWS:
            self.skipTest(
                "fake native binary is a POSIX shell script; Windows "
                "execution is covered by the CI smoke job with the real binary"
            )

    def current_platform_key(self):
        system = platform.system().lower()
        npm_os = {"linux": "linux", "windows": "win32", "darwin": "darwin"}.get(
            system, system
        )
        machine = platform.machine().lower()
        npm_cpu = {"x86_64": "x64", "amd64": "x64", "arm64": "arm64"}.get(
            machine, machine
        )
        return f"{npm_os}-{npm_cpu}"

    def bin_name(self) -> str:
        return "pumice.cmd" if IS_WINDOWS else "pumice"

    def local_bin(self, node_modules: Path) -> Path:
        return node_modules / ".bin" / self.bin_name()

    def global_bin(self, prefix: Path) -> Path:
        # npm puts global shims in <prefix>/bin on posix and in <prefix>
        # itself on Windows.
        if IS_WINDOWS and (prefix / self.bin_name()).exists():
            return prefix / self.bin_name()
        return prefix / "bin" / self.bin_name()

    def assert_wrong_platforms_skipped(self, node_modules: Path):
        # npm/Bun hoist the native package to node_modules/@brasillero;
        # pnpm keeps it in its isolated store and links it only into the
        # front package. Search the whole tree either way.
        installed = sorted(
            {
                path.name
                for path in node_modules.rglob("pumice-*")
                if path.is_dir() and path.parent.name == "@brasillero"
            }
        )
        expected = f"pumice-{self.current_platform_key()}"
        self.assertEqual(installed, [expected], node_modules)

    def run_cli(self, cli: Path, *args):
        result = subprocess.run(
            [str(cli), *args],
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result

    @unittest.skipIf(NPM is None, "npm not found (set PUMICE_NPM)")
    def test_npm_local_install_bin_and_platform_skip(self):
        result = self.run_tool(
            NPM,
            "install",
            f"pumice@{FRONT_VERSION}",
            "--registry",
            self.registry.base_url,
            "--no-save",
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        cli = self.local_bin(self.project / "node_modules")
        self.assertTrue(cli.exists(), f"no .bin entry at {cli}")
        self.require_fake_exec()
        output = self.run_cli(cli, "hello", "two words")
        self.assertIn("fake-pumice <hello> <two words>", output.stdout)
        self.assert_wrong_platforms_skipped(self.project / "node_modules")

    @unittest.skipIf(NPM is None, "npm not found (set PUMICE_NPM)")
    def test_npm_global_prefix_install(self):
        prefix = self.project / "prefix"
        result = self.run_tool(
            NPM,
            "install",
            "-g",
            f"pumice@{FRONT_VERSION}",
            "--registry",
            self.registry.base_url,
            "--prefix",
            str(prefix),
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        cli = self.global_bin(prefix)
        self.assertTrue(cli.exists(), f"no global bin at {cli}")
        self.require_fake_exec()
        output = self.run_cli(cli, "from-prefix")
        self.assertIn("fake-pumice <from-prefix>", output.stdout)

    @unittest.skipIf(NPM is None, "npm not found (set PUMICE_NPM)")
    @unittest.skipIf(IS_WINDOWS, "fake native binary is a POSIX shell script")
    def test_npx_runs_package_from_registry(self):
        result = self.run_tool(
            NPM,
            "exec",
            "--yes",
            f"--registry={self.registry.base_url}",
            "--",
            f"pumice@{FRONT_VERSION}",
            "via-npx",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("fake-pumice <via-npx>", result.stdout)

    @unittest.skipIf(PNPM is None, "pnpm not found (set PUMICE_PNPM)")
    def test_pnpm_local_install_bin(self):
        store = self.project / "pnpm-store"
        result = self.run_tool(
            PNPM,
            "add",
            f"pumice@{FRONT_VERSION}",
            f"--registry={self.registry.base_url}",
            f"--store-dir={store}",
            "--reporter=silent",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        cli = self.local_bin(self.project / "node_modules")
        self.assertTrue(cli.exists(), f"no .bin entry at {cli}")
        self.require_fake_exec()
        output = self.run_cli(cli, "via-pnpm")
        self.assertIn("fake-pumice <via-pnpm>", output.stdout)
        self.assert_wrong_platforms_skipped(self.project / "node_modules")

    @unittest.skipIf(BUN is None, "bun not found (set PUMICE_BUN)")
    def test_bun_local_install_bin(self):
        result = self.run_tool(
            BUN,
            "add",
            f"pumice@{FRONT_VERSION}",
            f"--registry={self.registry.base_url}",
            "--no-progress",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        cli = self.local_bin(self.project / "node_modules")
        self.assertTrue(cli.exists(), f"no .bin entry at {cli}")
        self.require_fake_exec()
        output = self.run_cli(cli, "via-bun")
        self.assertIn("fake-pumice <via-bun>", output.stdout)
        self.assert_wrong_platforms_skipped(self.project / "node_modules")


if __name__ == "__main__":
    unittest.main()
