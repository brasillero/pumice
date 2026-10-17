# License texts for the generated third-party notices

`scripts/third_party_notices.py` reads most license texts from the crate
sources Cargo downloaded for `Cargo.lock`. The texts below are not in any
crate, so they are kept here verbatim, fetched on 2026-10-16:

| File | Source | SHA-256 |
| --- | --- | --- |
| `Apache-2.0.txt` | <https://www.apache.org/licenses/LICENSE-2.0.txt> | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| `MIT.txt` | The MIT license's permission text without a copyright line (<https://opensource.org/license/mit>), for crates that publish no license file | `f9c4c77baa3828004ee54b8a4f2db2e88ed44a6237a493965bf551fac0fcb62d` |
| `musl-1.2.5-COPYRIGHT` | <https://git.musl-libc.org/cgit/musl/plain/COPYRIGHT?h=v1.2.5> (the `libc.a` the Rust `x86_64-unknown-linux-musl` target bundles reports 1.2.5) | `f9bc4423732350eb0b3f7ed7e91d530298476f8fec0c6c427a1c04ade22655af` |
| `Vulkan-Headers-1.4.350.0-LICENSE.md` | <https://raw.githubusercontent.com/KhronosGroup/Vulkan-Headers/vulkan-sdk-1.4.350.0/LICENSE.md> (the SDK the Vulkan modules are built with) | `ac24e5ea920e4318e4d02c4086ae51f53cfb03feed06c18df1019e7ada1ec7bc` |
| `SPIRV-Headers-1.4.350.0-LICENSE` | <https://raw.githubusercontent.com/KhronosGroup/SPIRV-Headers/vulkan-sdk-1.4.350.0/LICENSE> | `ea43b1de38a6f90c488800d66dec1ed671e68cda530266bc96951fb5b6307613` |

Update them together with the Vulkan SDK pin in the transcription build
actions (`.github/actions/`) or when the Rust musl target moves to another
musl release.
