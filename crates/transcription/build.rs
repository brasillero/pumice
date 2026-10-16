//! Build script for pumice-transcription.
//!
//! The default build is fully static and needs nothing here. The opt-in
//! `cpu-variants` feature links the engine as shared libraries (see
//! `transcribe-cpp/dynamic-backends`), so the staged binary must find those
//! libraries next to itself. On Linux an `$ORIGIN` rpath makes a relocated
//! binary resolve them from its own directory (see
//! `scripts/build-cpu-variants.sh` and `scripts/build-linux-vulkan.sh`); on
//! macOS an `@executable_path` rpath does the same for the core libraries'
//! `@rpath/` install names (see `scripts/build-macos-metal.sh`); Windows
//! resolves DLLs from the executable's directory first and needs nothing (see
//! `scripts/build-windows-vulkan.ps1`). The conservative portable baseline in
//! `.cargo/config.toml` is untouched by this.
//!
//! Three cfgs keep the target conditions in one place:
//! - `pumice_dynamic_backends`: `cpu-variants` on a target where startup
//!   loads the staged modules (Linux, x86-64 Windows, and Apple Silicon with
//!   `metal`);
//! - `pumice_vulkan`: `vulkan` on a target whose engine is built with the
//!   Vulkan module (x86-64 Windows, x86-64 Linux with glibc; never musl);
//! - `pumice_metal`: `metal` on a target whose engine is built with the Metal
//!   module (Apple Silicon macOS; never Intel).

fn main() {
    println!("cargo::rustc-check-cfg=cfg(pumice_dynamic_backends)");
    println!("cargo::rustc-check-cfg=cfg(pumice_vulkan)");
    println!("cargo::rustc-check-cfg=cfg(pumice_metal)");
    // The target triple a bundle manifest must name (src/bundle.rs).
    println!(
        "cargo:rustc-env=PUMICE_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );

    let cpu_variants = std::env::var_os("CARGO_FEATURE_CPU_VARIANTS").is_some();
    let vulkan = std::env::var_os("CARGO_FEATURE_VULKAN").is_some();
    let metal = std::env::var_os("CARGO_FEATURE_METAL").is_some();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let windows_x64 = target_os == "windows" && target_arch == "x86_64";
    // The targets Cargo.toml gives the native Vulkan build.
    let vulkan_target = target_arch == "x86_64"
        && (target_os == "windows" || (target_os == "linux" && target_env == "gnu"));
    // The target Cargo.toml gives the native Metal build.
    let metal_target = target_os == "macos" && target_arch == "aarch64";

    if cpu_variants && target_os == "linux" {
        // rustc invokes the linker without a shell, so the literal `$ORIGIN`
        // reaches the ELF rpath unchanged.
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
    if cpu_variants && target_os == "macos" {
        // The core libraries' install names are `@rpath/<name>`; this is the
        // executable's only LC_RPATH (the engine's own build-directory rpath
        // is a link argument of its -sys crate and does not reach here).
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
    }
    if cpu_variants && windows_x64 && target_env == "msvc" {
        // Loaded after the bundle check by full path from the executable's
        // folder (engine.rs, preload_core), then bound by the delay-load
        // helper (delayimp.lib) on the first engine call; a static import
        // would be resolved by the standard search order, which ends with
        // the current directory and PATH, before `main`.
        // The executable imports nothing else from the engine (the bundle
        // audit checks it).
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:transcribe.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    } else if cpu_variants && windows_x64 {
        println!(
            "cargo:warning=cpu-variants on Windows delay-loads the engine only with MSVC: \
             this build resolves transcribe.dll before main, with the standard DLL search order"
        );
    }
    if cpu_variants && (target_os == "linux" || windows_x64 || (metal && metal_target)) {
        println!("cargo:rustc-cfg=pumice_dynamic_backends");
    } else if cpu_variants {
        println!(
            "cargo:warning=cpu-variants is only wired up on Linux, x86-64 Windows and (with \
             metal) Apple Silicon: the build works, but the binary will not auto-load variant \
             modules on this target"
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
    if metal && metal_target {
        println!("cargo:rustc-cfg=pumice_metal");
    } else if metal {
        println!(
            "cargo:warning=metal is only wired up on Apple Silicon macOS: this build has the \
             CPU variants only"
        );
    }
}
