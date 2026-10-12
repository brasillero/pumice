#!/usr/bin/env python3
"""Embed musl libstdc++ and rescan Rust's self-contained libc/libunwind."""
import os
from pathlib import Path
import subprocess
import sys

# MUSL_TOOLCHAIN is the extracted archive's x86_64-unknown-linux-musl directory.
cc = str(Path(os.environ["MUSL_TOOLCHAIN"]) / "bin/x86_64-unknown-linux-musl-gcc")
lib = subprocess.check_output([cc, "-print-file-name=libstdc++.a"], text=True).strip()
if not Path(lib).is_file():
    sys.exit(f"missing static C++ library: {lib}")
args = [lib if arg == "-lstdc++" else arg for arg in sys.argv[1:]]
args += ["-Wl,-Bstatic", "-lc", "-lunwind"]
os.execv(cc, [cc, *args])
