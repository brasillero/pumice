"""Reject runtime dependencies other than macOS system libraries."""
import subprocess
import sys

binary, target = sys.argv[1:]
if target.endswith("linux-musl"):
    description = subprocess.check_output(["file", binary], text=True)
    print(description, end="")
    if "statically linked" not in description and "static-pie linked" not in description:
        sys.exit("FAIL: file did not report static linkage")
    result = subprocess.run(["ldd", binary], text=True, capture_output=True)
    output = result.stdout + result.stderr
    print(output, end="")
    if not any(s in output for s in ("statically linked", "not a dynamic executable")):
        sys.exit("FAIL: ldd did not report static linkage")
    headers = subprocess.check_output(["readelf", "-l", binary], text=True)
    dynamic = subprocess.check_output(["readelf", "-d", binary], text=True)
    if "INTERP" in headers or "(NEEDED)" in dynamic:
        sys.exit("FAIL: Linux binary needs a dynamic loader or library")
else:
    output = subprocess.check_output(["otool", "-L", binary], text=True)
    print(output, end="")
    libraries = [line.strip().split(" (", 1)[0] for line in output.splitlines()[1:]]
    if not libraries or any(not lib.startswith(("/usr/lib/", "/System/")) for lib in libraries):
        sys.exit("FAIL: macOS binary has non-system library dependencies")
print("OK: standalone executable linkage")
