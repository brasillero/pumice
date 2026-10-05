#!/usr/bin/env python3
"""Loopback test registry for validating the npm packages without publishing.

Serves the ``.tgz`` files found in a directory as a minimal read-only npm
registry on 127.0.0.1: packuments (``GET /<name>``) are derived from each
tarball's embedded package.json, and tarballs are served from
``GET /<name>/-/<file>.tgz``. Package managers can be pointed at it with
``--registry=http://127.0.0.1:<port>`` so installs exercise the real
resolution path (optional-dependency platform skipping, version pinning)
against the exact tarballs that would be published. Stdlib only.

Run standalone (CI starts it in the background):

    python3 scripts/npm_registry_fixture.py --root dist/npm --port 0

or import ``serve``/``TestRegistry`` from the test suite
(scripts/test_npm_install.py).
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import tarfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote

# Packument fields copied verbatim from each tarball's package.json.
MANIFEST_FIELDS = (
    "name",
    "version",
    "description",
    "license",
    "type",
    "bin",
    "engines",
    "os",
    "cpu",
    "files",
    "optionalDependencies",
)


def read_manifest(tarball: Path) -> dict:
    with tarfile.open(tarball) as archive:
        for member in archive.getmembers():
            if member.name == "package/package.json" and member.isreg():
                return json.loads(archive.extractfile(member).read())
    raise SystemExit(f"error: no package/package.json in {tarball}")


def build_packument(name: str, tarballs: dict[str, Path], base_url: str) -> dict:
    """Builds a packument for one package name from its tarball files."""
    versions = {}
    for filename, path in tarballs.items():
        manifest = read_manifest(path)
        payload = path.read_bytes()
        version_doc = {
            key: manifest[key] for key in MANIFEST_FIELDS if key in manifest
        }
        version_doc["dist"] = {
            "tarball": f"{base_url}/{name}/-/{filename}",
            "integrity": "sha512-"
            + base64.b64encode(hashlib.sha512(payload).digest()).decode(),
            "shasum": hashlib.sha1(payload).hexdigest(),
        }
        versions[manifest["version"]] = version_doc
    return {
        "name": name,
        "dist-tags": {"latest": max(versions)},
        "versions": versions,
    }


def index_tarballs(root: Path, base_url: str) -> dict[str, dict]:
    """Maps npm package name -> packument for every .tgz in root."""
    files = sorted(root.glob("*.tgz"))
    if not files:
        raise SystemExit(f"error: no .tgz files found in {root}")
    by_name: dict[str, dict[str, Path]] = {}
    for path in files:
        by_name.setdefault(read_manifest(path)["name"], {})[path.name] = path
    return {
        name: build_packument(name, tarballs, base_url)
        for name, tarballs in by_name.items()
    }


class TestRegistry:
    """A loopback-only npm registry serving one directory of tarballs."""

    def __init__(self, root: Path, port: int = 0):
        self.root = Path(root)
        host = "127.0.0.1"
        self._httpd = ThreadingHTTPServer((host, port), self._handler())
        self.base_url = f"http://{host}:{self._httpd.server_address[1]}"
        self._index = index_tarballs(self.root, self.base_url)
        self._files = {
            name: set(tarballs)
            for name, tarballs in self._grouped().items()
        }
        self._thread: threading.Thread | None = None

    def _grouped(self) -> dict[str, dict[str, Path]]:
        grouped: dict[str, dict[str, Path]] = {}
        for path in sorted(self.root.glob("*.tgz")):
            grouped.setdefault(read_manifest(path)["name"], {})[path.name] = path
        return grouped

    @property
    def port(self) -> int:
        return self._httpd.server_address[1]

    def start(self) -> None:
        self._thread = threading.Thread(
            target=self._httpd.serve_forever, daemon=True, name="test-registry"
        )
        self._thread.start()

    def stop(self) -> None:
        self._httpd.shutdown()
        self._httpd.server_close()
        if self._thread:
            self._thread.join(timeout=5)

    def _handler(self):
        registry = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, format, *args):  # noqa: A002 - stdlib name
                pass

            def _send(self, status: int, body: bytes, content_type: str):
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(body)

            def _lookup(self):
                # npm requests scoped names as /@scope%2fname; unquote both
                # the %2f and a literal slash separator.
                name = unquote(self.path.split("?", 1)[0].lstrip("/"))
                if name in registry._index:
                    return ("packument", name)
                if "/-/" in name:
                    package_name, _, filename = name.rpartition("/-/")
                    if package_name in registry._index and filename:
                        return ("tarball", (package_name, filename))
                return None

            def do_HEAD(self):
                self.do_GET()

            def do_GET(self):
                found = self._lookup()
                if not found:
                    self._send(404, b'{"error":"not found"}', "application/json")
                    return
                kind, value = found
                if kind == "packument":
                    body = json.dumps(registry._index[value]).encode()
                    self._send(200, body, "application/json")
                    return
                package_name, filename = value
                if (
                    filename not in registry._files[package_name]
                    or not (registry.root / filename).is_file()
                ):
                    self._send(404, b'{"error":"not found"}', "application/json")
                    return
                self._send(
                    200,
                    (registry.root / filename).read_bytes(),
                    "application/octet-stream",
                )

        return Handler


def serve(root: Path, port: int = 0) -> TestRegistry:
    registry = TestRegistry(root, port=port)
    registry.start()
    return registry


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--root", type=Path, required=True, help="directory of .tgz files")
    parser.add_argument("--port", type=int, default=0, help="port (0 = pick a free one)")
    args = parser.parse_args(argv)

    registry = TestRegistry(args.root, port=args.port)
    print(f"pumice test registry listening on {registry.base_url}", flush=True)
    try:
        registry._httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
