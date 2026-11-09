//! Keeps this process's standard handles out of the children it spawns
//! (Windows; a no-op elsewhere).
//!
//! Windows children are created with handle inheritance on, so a child
//! receives every inheritable handle of its parent, not only the stdio it
//! was given. [`ProcessTree`](super::ProcessTree) creates a child suspended,
//! assigns it to the Job Object and only then resumes it; a parent killed in
//! between leaves the child suspended outside any job, never to run or exit.
//! If the parent's own stdout and stderr were inheritable, that child holds
//! them, and whoever reads the parent's output waits forever
//! (chore-cleanup-test-bounds/D4).
//!
//! Clearing the inherit flag on the standard handles at startup closes that
//! door without changing what children see: `Stdio::inherit()` hands a child
//! an inheritable duplicate of the standard handle made for that spawn, and
//! piped or null stdio never involves these handles.

/// Makes stdin, stdout and stderr non-inheritable on Windows, before any
/// child is spawned. Missing or invalid handles (no console, detached) are
/// skipped and failures are ignored: startup never fails because of this.
/// Safe to repeat. Does nothing on other platforms, where the spawn path
/// has no such window.
pub fn make_std_handles_non_inheritable() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: `GetStdHandle` only reads the process's standard handle
            // table; it returns null or `INVALID_HANDLE_VALUE` when there is
            // none, which `clear_inherit` skips.
            clear_inherit(unsafe { GetStdHandle(id) });
        }
    }
}

#[cfg(windows)]
fn clear_inherit(handle: windows_sys::Win32::Foundation::HANDLE) {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return;
    }
    // SAFETY: `handle` is one of this process's standard handles; changing
    // its inherit flag touches no memory, and a handle that cannot take the
    // flag (an old-style console pseudo-handle) only makes the call fail.
    let _ = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_idempotent() {
        make_std_handles_non_inheritable();
        make_std_handles_non_inheritable();

        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};
            use windows_sys::Win32::System::Console::{
                GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
            };
            for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                // SAFETY: as in `make_std_handles_non_inheritable`.
                let handle = unsafe { GetStdHandle(id) };
                let mut flags = 0u32;
                // SAFETY: `flags` is a valid, writable u32; an absent or
                // invalid handle only makes the call fail.
                if unsafe { GetHandleInformation(handle, &mut flags) } != 0 {
                    assert_eq!(
                        flags & HANDLE_FLAG_INHERIT,
                        0,
                        "std handle {id} inheritable"
                    );
                }
            }
        }
    }

    /// What `GetStdHandle` returns with no console or a detached process.
    #[cfg(windows)]
    #[test]
    fn skips_missing_and_invalid_handles() {
        clear_inherit(std::ptr::null_mut());
        clear_inherit(windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE);
    }
}
