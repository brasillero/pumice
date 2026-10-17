#!/usr/bin/env python3
"""Development-only generator for Pumice's combined third-party notices.

Writes one ``THIRD_PARTY_NOTICES.md`` per platform package covering what that
package ships:

- the Rust crates built into ``pumice`` for the package's core target, and
  (unless ``--core-only``) into ``pumice-transcribe`` for its transcription
  bundle target and features. The crate graph comes from ``cargo metadata
  --locked --filter-platform <target>``: every package reachable from the
  binary's package through normal (non-dev, non-build) dependencies, minus
  the Pumice workspace members. Build-only tools never ship and are left out;
  proc-macros are kept, which errs on the side of listing too much.
- the native components compiled into those files that no crate's own
  license file covers (transcribe.cpp, ggml and the code ggml embeds, libopus,
  the Vulkan SDK headers, musl, the Rust standard library), in ``NATIVE``.
- the existing notices for the Handy-derived formatter prompt.

License texts are read from each crate's downloaded source (root files named
LICENSE*, LICENCE*, COPYING*, NOTICE*, COPYRIGHT*, UNLICENSE*, or the
manifest's ``license-file``); identical texts are printed once. A crate that
publishes no license file gets the standard text of its declared license from
``scripts/notices/`` and its authors as the copyright holder, and is flagged
as such. Unknown declared licenses without a file fail the run.

Run ``cargo fetch --locked`` first so every crate source is on disk (``cargo
metadata`` downloads them too). Used by .github/workflows/release.yml; tests
in scripts/test_third_party_notices.py. Stdlib only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import package_npm
import package_release

REPO_ROOT = Path(__file__).resolve().parent.parent
TEXTS_DIR = Path(__file__).resolve().parent / "notices"
FORMATTER_NOTICES = REPO_ROOT / "crates" / "formatter" / "THIRD_PARTY_NOTICES.md"

CORE_PACKAGE = "pumice"
TRANSCRIPTION_PACKAGE = "pumice-transcription"
WORKSPACE_PACKAGES = frozenset(
    {"pumice", "pumice-relay", "pumice-formatter", "pumice-transcription"}
)

LICENSE_FILE = re.compile(
    r"^(licen[cs]e|copying|notice|copyright|unlicense)([-_.].*)?$", re.IGNORECASE
)

# Standard texts used only for crates that ship no license file.
FALLBACK_TEXTS = {"MIT": "MIT.txt", "Apache-2.0": "Apache-2.0.txt"}


def _crate_file(crate: str, path: str) -> dict:
    return {"kind": "crate", "crate": crate, "path": path}


def _crate_header(crate: str, path: str) -> dict:
    """The leading ``//`` comment block of a source file in a crate."""
    return {"kind": "header", "crate": crate, "path": path}


def _repo_file(name: str) -> dict:
    return {"kind": "repo", "path": name}


def _mit(copyright_line: str) -> dict:
    return {"kind": "mit", "copyright": copyright_line}


# Native code no crate-level license file covers. `when` names what must be
# shipped for the entry to apply: "core" (pumice), "transcription" (any
# transcription bundle), a bundle feature ("vulkan", "metal") or a core
# target triple.
NATIVE = [
    {
        "name": "Rust standard library",
        "license": "MIT OR Apache-2.0",
        "when": "core",
        "where": "linked into every Rust executable in this package",
        "texts": [
            _mit("Copyright (c) The Rust Project Contributors"),
            _repo_file("Apache-2.0.txt"),
        ],
    },
    {
        "name": "musl libc 1.2.5",
        "license": "MIT",
        "when": "x86_64-unknown-linux-musl",
        "where": "statically linked into `pumice` (the Rust musl target's `libc.a`)",
        "texts": [_repo_file("musl-1.2.5-COPYRIGHT")],
    },
    {
        "name": "transcribe.cpp 0.3.1 (transcribe-cpp-sys)",
        "license": "MIT",
        "when": "transcription",
        "where": "the transcribe core library",
        "texts": [_crate_file("transcribe-cpp-sys", "LICENSE")],
    },
    {
        "name": "ggml (vendored in transcribe-cpp-sys 0.3.1)",
        "license": "MIT",
        "when": "transcription",
        "where": "the ggml core libraries, CPU modules and GPU module",
        "texts": [_crate_file("transcribe-cpp-sys", "ggml/LICENSE")],
    },
    {
        "name": "llamafile sgemm (in ggml's CPU backend)",
        "license": "MIT",
        "when": "transcription",
        "where": "the ggml CPU modules",
        "texts": [
            _crate_header(
                "transcribe-cpp-sys", "ggml/src/ggml-cpu/llamafile/sgemm.cpp"
            )
        ],
    },
    {
        "name": "YaRN rotary embedding (in ggml's CPU and Metal backends)",
        "license": "MIT",
        "when": "transcription",
        "where": "the ggml CPU modules and the Metal module",
        "texts": [_mit("Copyright (c) 2023 Jeffrey Quesnelle and Bowen Peng")],
    },
    {
        "name": "miniz (vendored in transcribe.cpp)",
        "license": "MIT",
        "when": "transcription",
        "where": "the transcribe core library",
        "texts": [_crate_file("transcribe-cpp-sys", "src/third_party/miniz/LICENSE")],
    },
    {
        "name": "libopus 1.6.1 (bundled by opusic-sys 0.7.5)",
        "license": "BSD-3-Clause",
        "when": "transcription",
        "where": "statically linked into `pumice-transcribe`",
        "texts": [_crate_file("opusic-sys", "opus/COPYING")],
    },
    {
        "name": "Vulkan-Headers and Vulkan-Hpp (Vulkan SDK 1.4.350.0)",
        "license": "Apache-2.0 (vulkan_core.h; vulkan.hpp is Apache-2.0 OR MIT)",
        "when": "vulkan",
        "where": "compiled into the Vulkan GPU module",
        "texts": [
            {
                "kind": "inline",
                "text": "Copyright 2015-2026 The Khronos Group Inc.\n\n"
                "SPDX-License-Identifier: Apache-2.0 (include/vulkan/vulkan_core.h)\n"
                "SPDX-License-Identifier: Apache-2.0 OR MIT (include/vulkan/vulkan.hpp)\n",
            },
            _repo_file("Vulkan-Headers-1.4.350.0-LICENSE.md"),
            _repo_file("Apache-2.0.txt"),
        ],
    },
    {
        "name": "SPIRV-Headers (Vulkan SDK 1.4.350.0)",
        "license": "MIT (Khronos)",
        "when": "vulkan",
        "where": "compiled into the Vulkan GPU module",
        "texts": [_repo_file("SPIRV-Headers-1.4.350.0-LICENSE")],
    },
]

TOOLCHAIN_NOTE = (
    "Compiler and C runtime support code the toolchains link in (the static "
    "MSVC runtime on Windows, LLVM libunwind in the Linux `pumice`, the GCC "
    "and Clang runtime libraries) is distributed under terms that require no "
    "notice in binary form (the Microsoft Visual C++ redistribution terms, "
    "the LLVM exception, the GCC Runtime Library Exception). Operating system "
    "libraries the files load at runtime (glibc, libstdc++, the Vulkan "
    "loader, Apple's system frameworks) are not shipped."
)


def cargo_metadata(target: str, features: list[str], cargo: str, repo_root: Path) -> dict:
    command = [
        cargo,
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--filter-platform",
        target,
    ]
    if features:
        command += ["--features", ",".join(features)]
    result = subprocess.run(
        command, cwd=repo_root, capture_output=True, text=True
    )
    if result.returncode != 0:
        raise SystemExit(
            f"error: {' '.join(command)} failed:\n{result.stderr}"
        )
    return json.loads(result.stdout)


def shipped_packages(metadata: dict, root_name: str) -> list[dict]:
    """Third-party packages reachable from ``root_name`` by normal deps."""
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    roots = [
        package["id"]
        for package in metadata["packages"]
        if package["name"] == root_name and package["id"] in nodes
    ]
    if len(roots) != 1:
        raise SystemExit(f"error: package {root_name} not found in cargo metadata")
    seen: set[str] = set()
    stack = roots
    while stack:
        package_id = stack.pop()
        if package_id in seen:
            continue
        seen.add(package_id)
        for dep in nodes[package_id]["deps"]:
            if any(kind.get("kind") is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    return sorted(
        (
            packages[package_id]
            for package_id in seen
            if packages[package_id]["name"] not in WORKSPACE_PACKAGES
        ),
        key=lambda package: (package["name"], package["version"]),
    )


def crate_dir(package: dict) -> Path:
    return Path(package["manifest_path"]).parent


def _read_text(path: Path) -> str:
    return path.read_bytes().decode("utf-8", errors="replace").replace("\r\n", "\n")


def license_texts(package: dict) -> tuple[list[tuple[str, str]], bool]:
    """Returns ([(file name, text)], used_fallback) for one crate."""
    directory = crate_dir(package)
    found = sorted(
        path
        for path in directory.iterdir()
        if path.is_file() and not path.is_symlink() and LICENSE_FILE.match(path.name)
    )
    license_file = package.get("license_file")
    if license_file:
        path = (directory / license_file).resolve()
        if directory.resolve() in path.parents and path.is_file() and path not in found:
            found.append(path)
    texts = [(path.name, _read_text(path)) for path in found]
    if texts:
        return texts, False

    expression = package.get("license") or ""
    options = [
        token for token in re.split(r"\s+OR\s+|/", expression) if token.strip()
    ]
    for option in options:
        option = option.strip("() ")
        if option in FALLBACK_TEXTS:
            body = _read_text(TEXTS_DIR / FALLBACK_TEXTS[option])
            authors = ", ".join(package.get("authors") or []) or package["name"]
            if option == "MIT":
                body = f"Copyright (c) {authors}\n\n{body}"
            return [(f"standard {option} text", body)], True
    raise SystemExit(
        f"error: {package['name']} {package['version']} ships no license file "
        f"and its license ({expression or 'none'}) has no standard text in "
        f"{TEXTS_DIR}; add one"
    )


def native_text(source: dict, crates: dict[str, Path]) -> str:
    kind = source["kind"]
    if kind == "inline":
        return source["text"]
    if kind == "mit":
        return f"{source['copyright']}\n\n{_read_text(TEXTS_DIR / 'MIT.txt')}"
    if kind == "repo":
        return _read_text(TEXTS_DIR / source["path"])
    directory = crates.get(source["crate"])
    if directory is None:
        raise SystemExit(
            f"error: crate {source['crate']} is not in the shipped graph; "
            "update NATIVE in scripts/third_party_notices.py"
        )
    path = directory / source["path"]
    if not path.is_file():
        raise SystemExit(f"error: native license source missing: {path}")
    text = _read_text(path)
    if kind == "crate":
        return text
    lines = []
    for line in text.splitlines():
        if not line.startswith("//"):
            break
        lines.append(line[2:].removeprefix(" "))
    if not any("copyright" in line.lower() for line in lines):
        raise SystemExit(f"error: no license header at the top of {path}")
    return "\n".join(lines).strip() + "\n"


def _fence(text: str) -> str:
    longest = max((len(run) for run in re.findall(r"`+", text)), default=0)
    fence = "`" * max(3, longest + 1)
    return f"{fence}text\n{text.rstrip()}\n{fence}"


def _formatter_section() -> str:
    lines = _read_text(FORMATTER_NOTICES).splitlines()
    body = [line for line in lines if not line.startswith("# ")]
    return "\n".join(
        ("#" + line) if line.startswith("## ") else line for line in body
    ).strip()


def components(platform_key: str, core_only: bool) -> list[dict]:
    """What a package ships: [{binary, package, target, features}]."""
    core_target = {
        key: target for target, (key, _exe) in package_npm.TARGET_TO_NPM.items()
    }.get(platform_key)
    if core_target is None:
        raise SystemExit(f"error: unknown platform {platform_key}")
    shipped = [
        {
            "binary": "pumice",
            "package": CORE_PACKAGE,
            "target": core_target,
            "features": [],
        }
    ]
    bundle = package_npm.BUNDLES.get(platform_key)
    if bundle is not None and not core_only:
        shipped.append(
            {
                "binary": "pumice-transcribe",
                "package": TRANSCRIPTION_PACKAGE,
                "target": bundle["target"],
                "features": list(bundle["features"]),
            }
        )
    return shipped


def generate(
    platform_key: str,
    core_only: bool = False,
    cargo: str = "cargo",
    repo_root: Path = REPO_ROOT,
) -> str:
    version = package_release.crate_version(repo_root)
    shipped = components(platform_key, core_only)

    crates: dict[tuple[str, str], dict] = {}
    used_in: dict[tuple[str, str], list[str]] = {}
    for item in shipped:
        features = [f"{item['package']}/{feature}" for feature in item["features"]]
        metadata = cargo_metadata(item["target"], features, cargo, repo_root)
        for package in shipped_packages(metadata, item["package"]):
            key = (package["name"], package["version"])
            crates.setdefault(key, package)
            used_in.setdefault(key, []).append(f"`{item['binary']}`")
    crate_dirs = {name: crate_dir(package) for (name, _), package in crates.items()}

    active = {"core"}
    for item in shipped:
        if item["binary"] == "pumice":
            active.add(item["target"])
        else:
            active.add("transcription")
            active.update(item["features"])

    # Identical texts are printed once, numbered in first-use order.
    text_ids: dict[str, int] = {}
    text_bodies: list[tuple[str, list[str]]] = []

    def text_ref(body: str, user: str) -> int:
        digest = hashlib.sha256(body.strip().encode()).hexdigest()
        if digest not in text_ids:
            text_ids[digest] = len(text_bodies) + 1
            text_bodies.append((body, []))
        number = text_ids[digest]
        if user not in text_bodies[number - 1][1]:
            text_bodies[number - 1][1].append(user)
        return number

    described = ", ".join(
        f"`{item['binary']}` ({item['target']}"
        + (f", features: {', '.join(item['features'])}" if item["features"] else "")
        + ")"
        for item in shipped
    )
    out = [
        f"# Third-party notices — Pumice {version} ({platform_key})",
        "",
        f"This package ships {described}. Pumice itself is MIT-licensed "
        "(`LICENSE`). Below are the third-party components built into those "
        "files and their license texts.",
        "",
        "Generated by `scripts/third_party_notices.py` from `Cargo.lock` "
        "(`cargo metadata --locked --filter-platform <target>`, normal "
        "dependencies of each binary) and the native component list in that "
        "script.",
        "",
        TOOLCHAIN_NOTE,
        "",
        "## Native components",
        "",
    ]
    for entry in NATIVE:
        if entry["when"] not in active:
            continue
        numbers = [
            text_ref(native_text(source, crate_dirs), entry["name"])
            for source in entry["texts"]
        ]
        refs = ", ".join(f"[text {n}](#text-{n})" for n in numbers)
        out.append(
            f"- **{entry['name']}** — {entry['license']}; {entry['where']}. "
            f"License: {refs}."
        )
    out += ["", "## Adapted content", "", _formatter_section(), ""]

    out += [
        "## Rust crates",
        "",
        f"{len(crates)} crates.",
        "",
        "| Crate | Version | License | Shipped in | License texts |",
        "| --- | --- | --- | --- | --- |",
    ]
    fallbacks = []
    for key in sorted(crates):
        package = crates[key]
        texts, fallback = license_texts(package)
        user = f"{key[0]} {key[1]}"
        numbers = []
        for name, body in texts:
            numbers.append(text_ref(body, user))
        if fallback:
            fallbacks.append(user)
        refs = ", ".join(f"[{n}](#text-{n})" for n in numbers)
        if fallback:
            refs += " (no license file published; standard text)"
        license_expr = (package.get("license") or "see text").replace("|", "\\|")
        out.append(
            f"| {key[0]} | {key[1]} | {license_expr} | "
            f"{', '.join(dict.fromkeys(used_in[key]))} | {refs} |"
        )
    out.append("")
    if fallbacks:
        out += [
            "Crates without a published license file (their declared license's "
            "standard text, with the crate's authors as the copyright holder): "
            + ", ".join(fallbacks)
            + ".",
            "",
        ]

    out += ["## License texts", ""]
    for number, (body, users) in enumerate(text_bodies, start=1):
        shown = ", ".join(users[:12])
        if len(users) > 12:
            shown += f" and {len(users) - 12} more"
        out += [
            f'<a id="text-{number}"></a>',
            f"### Text {number}",
            "",
            f"Used by: {shown}.",
            "",
            _fence(body),
            "",
        ]
    return "\n".join(out).rstrip() + "\n"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--platform",
        action="append",
        choices=sorted(key for key, _exe in package_npm.TARGET_TO_NPM.values()),
        help="npm platform key (repeatable; default: all four)",
    )
    parser.add_argument(
        "--core-only",
        action="store_true",
        help="cover `pumice` only (the release archives), not the bundle",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        required=True,
        help="writes <platform>.md for each platform here",
    )
    parser.add_argument("--cargo", default="cargo", help="cargo executable")
    args = parser.parse_args(argv)

    platforms = args.platform or sorted(
        key for key, _exe in package_npm.TARGET_TO_NPM.values()
    )
    args.output_dir.mkdir(parents=True, exist_ok=True)
    for platform_key in platforms:
        path = args.output_dir / f"{platform_key}.md"
        path.write_text(generate(platform_key, args.core_only, args.cargo))
        print(f"{path} ({path.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
