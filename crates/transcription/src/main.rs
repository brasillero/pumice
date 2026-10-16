use pumice_transcription::cli;
use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(windows)]
    if let Err(error) = restrict_dll_search() {
        eprintln!(
            "pumice-transcribe: cannot restrict the DLL search path (SetDefaultDllDirectories \
             failed: {error})"
        );
        return ExitCode::FAILURE;
    }
    cli::run(std::env::args_os())
}

/// Limits every later DLL lookup of this process to the executable's
/// directory and System32: never the current directory, PATH or the Windows
/// directory. It covers loads by name, the dependencies of every DLL (the
/// CPU and Vulkan modules ggml loads by full path with a plain
/// `LoadLibraryW`), and `vulkan-1.dll`, which therefore comes from System32
/// or not at all. The core `transcribe.dll` is loaded with its own,
/// equally narrow flags after the bundle check (engine.rs, preload_core).
/// KnownDLLs and API sets are resolved by the system before any search.
/// Runs first in `main`, before anything can load a DLL; the setting cannot
/// be undone and lasts for the life of the process. Available on every
/// Windows that Rust's MSVC target supports (Windows 10 and later).
///
/// <https://learn.microsoft.com/windows/win32/api/libloaderapi/nf-libloaderapi-setdefaultdlldirectories>
/// <https://learn.microsoft.com/windows/win32/dlls/dynamic-link-library-search-order>
#[cfg(windows)]
fn restrict_dll_search() -> std::io::Result<()> {
    use windows_sys::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
    };
    // SAFETY: takes and returns plain integers and changes only this
    // process's DLL search settings.
    let ok = unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32)
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
