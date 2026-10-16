//! Build script for pumice-transcription.
//!
//! The default build is fully static and needs nothing here. The opt-in
//! `cpu-variants` feature links the engine as shared libraries (see
//! `transcribe-cpp/dynamic-backends`), so the staged binary must find those
//! libraries next to itself. On Linux an `$ORIGIN` rpath makes a relocated
//! binary resolve them from its own directory (see
//! `scripts/build-cpu-variants.sh` and `scripts/build-linux-vulkan.sh`);
//! Windows resolves DLLs from the executable's directory first and needs
//! nothing (see `scripts/build-windows-vulkan.ps1`). The conservative
//! portable baseline in `.cargo/config.toml` is untouched by this.
//!
//! Two cfgs keep the target conditions in one place:
//! - `pumice_dynamic_backends`: `cpu-variants` on a target where startup
//!   loads the staged modules (Linux, x86-64 Windows);
//! - `pumice_vulkan`: `vulkan` on a target whose engine is built with the
//!   Vulkan module (x86-64 Windows, x86-64 Linux with glibc; never musl).

fn main() {
    println!("cargo::rustc-check-cfg=cfg(pumice_dynamic_backends)");
    println!("cargo::rustc-check-cfg=cfg(pumice_vulkan)");
    // The target triple a bundle manifest must name (src/bundle.rs).
    println!(
        "cargo:rustc-env=PUMICE_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );

    let cpu_variants = std::env::var_os("CARGO_FEATURE_CPU_VARIANTS").is_some();
    let vulkan = std::env::var_os("CARGO_FEATURE_VULKAN").is_some();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let windows_x64 = target_os == "windows" && target_arch == "x86_64";
    // The targets Cargo.toml gives the native Vulkan build.
    let vulkan_target = target_arch == "x86_64"
        && (target_os == "windows" || (target_os == "linux" && target_env == "gnu"));

    if cpu_variants && target_os == "linux" {
        // rustc invokes the linker without a shell, so the literal `$ORIGIN`
        // reaches the ELF rpath unchanged.
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
    if cpu_variants && (target_os == "linux" || windows_x64) {
        println!("cargo:rustc-cfg=pumice_dynamic_backends");
    } else if cpu_variants {
        println!(
            "cargo:warning=cpu-variants is only wired up on Linux and x86-64 Windows: the \
             build works, but the binary will not auto-load variant modules on this target"
        );
    }
    if vulkan && vulkan_target {
        println!("cargo:rustc-cfg=pumice_vulkan");
    } else if vulkan {
        println!(
            "cargo:warning=vulkan is only wired up on x86-64 Windows and x86-64 Linux with \
             glibc: this build has the CPU variants only"
        );
    }
}
