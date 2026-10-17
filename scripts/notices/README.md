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

## Rust standard library (`rust-1.99.1/`)

The texts for the standard library of the toolchain the release preview
builds every shipped binary with: rustc 1.99.1, `RUST_VERSION` in
`scripts/third_party_notices.py`. Beyond its MIT OR
Apache-2.0 code, std carries separately licensed parts: the Unicode tables in `library/core/src/unicode` (Unicode-3.0, from
the toolchain's `share/doc/rust/COPYRIGHT-library.html` and the tag's
`REUSE.toml`), `compiler-builtins` with its `libm`, and the crates.io
crates std links. Their versions and targets come from the 1.99.0 tag's
`library/Cargo.lock` and `library/std/Cargo.toml`, cross-checked against
the path strings in the installed `x86_64-unknown-linux-gnu` rlibs; the
toolchain's own `COPYRIGHT-library.html` lists other versions of several of
them (memchr 2.8.0, no addr2line 0.27.1), so it is not used for the crates.
1.99.1 changed none of them: its `library/Cargo.lock`,
`library/std/Cargo.toml`, `REUSE.toml` and the three in-tree license files
are byte-identical to 1.99.0's (compared 2026-10-17). Fetched on
2026-10-16; `RAW` is `https://raw.githubusercontent.com/rust-lang/rust/1.99.0`
and `CRATE(n, v)` is `https://static.crates.io/crates/n/n-v.crate`:

| File | Source | SHA-256 |
| --- | --- | --- |
| `Unicode-3.0.txt` | `RAW/LICENSES/Unicode-3.0.txt` (identical to the toolchain's `share/doc/rust/licenses/Unicode-3.0.txt`) | `f5062c9a188d81dfe66b56db4182dcf9e4b17c0d9b0d311a8e20b3a1b075c443` |
| `compiler-builtins-LICENSE.txt` | `RAW/library/compiler-builtins/LICENSE.txt` | `ab6eec6caf0fa5775e411c7a8bc6a45c4ef2956b0980b157ab74fc5cd62a928b` |
| `libm-LICENSE.txt` | `RAW/library/compiler-builtins/libm/LICENSE.txt` | `3823dda7cf046602f4b4e77ec8e227863dc4736037cc85bb33d9f19febe16bb7` |
| `addr2line-0.27.1-LICENSE-MIT` | `LICENSE-MIT` in `CRATE(addr2line, 0.27.1)` | `e99d88d232bf57d70f0fb87f6b496d44b6653f99f8a63d250a54c61ea4bcde40` |
| `adler2-2.0.1-LICENSE-0BSD` | `LICENSE-0BSD` in `CRATE(adler2, 2.0.1)` | `861399f8c21c042b110517e76dc6b63a2b334276c8cf17412fc3c8908ca8dc17` |
| `cfg-if-1.0.4-LICENSE-MIT` | `LICENSE-MIT` in `CRATE(cfg-if, 1.0.4)` | `378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397` |
| `gimli-0.34.0-LICENSE-MIT` | `LICENSE-MIT` in `CRATE(gimli, 0.34.0)` | `7b63ecd5f1902af1b63729947373683c32745c16a10e8e6292e2e2dcd7e90ae0` |
| `hashbrown-0.17.1-LICENSE-MIT` | `LICENSE-MIT` in `CRATE(hashbrown, 0.17.1)` | `ff8f68cb076caf8cefe7a6430d4ac086ce6af2ca8ce2c4e5a2004d4552ef52a2` |
| `libc-0.2.189-LICENSE-MIT` | `LICENSE-MIT` in `CRATE(libc, 0.2.189)` | `123a331b5dbf04c30097fa43b8f858bc85df671fe776de498d01f3d6b7c1f69e` |
| `memchr-2.8.3-LICENSE-MIT` | `LICENSE-MIT` in `CRATE(memchr, 2.8.3)` | `0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| `miniz_oxide-0.9.1-LICENSE-MIT.md` | `LICENSE-MIT.md` in `CRATE(miniz_oxide, 0.9.1)` | `799e9ca9d179295ef372f25d3769cdda7d25bb2668add6a6a1e22d1e4c678b8d` |
| `object-0.39.1-LICENSE-MIT` | `LICENSE-MIT` in `CRATE(object, 0.39.1)` | `0b74dfa0bcee5c420c6b7f67b4b2658f9ab8388c97b8e733975f2cecbdd668a6` |
| `rustc-demangle-0.1.28-LICENSE-MIT` | `LICENSE-MIT` in `CRATE(rustc-demangle, 0.1.28)` | `378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397` |

### The release Rust version

`RUST_VERSION` is the single source of the Rust version of shipped binaries
(`p14.3-npm-bundles/D4`). The release preview's `package-tests` job reads it
with `python3 scripts/third_party_notices.py --print-rust-version`, and every
job that builds a shipped binary installs exactly that toolchain: the four
core builds, the npm notices job, and the three transcription bundle jobs,
which pass it to the bundle actions in `.github/actions/` as their
`rust-toolchain` input. The transcription and CI workflows keep following
stable (the actions default to `stable`). `scripts/third_party_notices.py`
fails when `rustc --version` is not `RUST_VERSION`, so the notices cannot
silently cover a different standard library.

To move release builds to a newer Rust, in one commit:

1. Fetch `library/Cargo.lock`, `library/std/Cargo.toml` and `REUSE.toml` at
   the new tag, and update `RUST_STD_CRATES` (versions, and targets from
   std's `[target]` tables) and the in-tree entries in `rust_std_entries()`.
2. Re-fetch the texts above into `rust-<version>/`, delete the old folder
   and update the table and its heading here.
3. Set `RUST_VERSION` to the new version. Nothing in the workflows changes.
4. Dispatch the release preview on the branch.

The release preview's bundle jobs do not share their Rust caches with the
transcription workflow's, although the job IDs and cache keys match:
`Swatinem/rust-cache` hashes the version of every installed toolchain, and
the pinned jobs also have the runner image's preinstalled stable (rustc
1.99.0 on the first pinned run, next to 1.99.1, while the transcription
workflow's `stable` update replaces it). So each release preview restores
only release-preview caches. The cost is one more set of bundle caches per
branch in the repository's cache storage; the first release preview after a
bump or a runner image update builds the bundles without a cache.
