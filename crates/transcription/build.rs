//! Build script for pumice-transcription.
//!
//! The default build is fully static and needs nothing here. The opt-in
//! `cpu-variants` feature links the engine as shared libraries (see
//! `transcribe-cpp/dynamic-backends`), so the staged binary must find those
//! libraries next to itself. On Linux an `$ORIGIN` rpath makes a relocated
//! binary resolve them from its own directory (see
//! `scripts/build-cpu-variants.sh`); the conservative portable baseline in
//! `.cargo/config.toml` is untouched by this.

fn main() {
    let cpu_variants = std::env::var_os("CARGO_FEATURE_CPU_VARIANTS").is_some();
    if cpu_variants && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // rustc invokes the linker without a shell, so the literal `$ORIGIN`
        // reaches the ELF rpath unchanged.
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
    if cpu_variants && std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        println!(
            "cargo:warning=cpu-variants is only wired up on Linux: the build works, \
             but the binary will not auto-load variant modules on this target"
        );
    }
}
