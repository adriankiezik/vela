//! Small platform shims for the double-click launch experience.
//!
//! When the executable is started from Explorer (double-clicked) rather than
//! from an existing terminal, two things go wrong for a console server:
//!   * the current working directory is arbitrary (often `system32` or the
//!     Desktop), so vanilla's "read runtime files from the CWD" behavior lands
//!     the world / `eula.txt` somewhere unexpected or unwritable; and
//!   * the console is created for this process alone, so it closes the instant
//!     the process exits — the user never sees the startup log, in particular
//!     the notice that they must agree to the EULA.
//!
//! [`owns_console`] detects that launch mode so the rest of the server can root
//! runtime files next to the exe and pause before exiting.

/// Whether this process *owns* (created) its console — i.e. it was launched by
/// double-clicking the executable rather than from an existing shell. When true,
/// the console window closes the moment the process exits.
///
/// Implemented via `kernel32!GetConsoleProcessList`, which reports the PIDs
/// attached to the current console: a console freshly created for us has exactly
/// one attached process (this one); a console inherited from a shell (cmd,
/// PowerShell, a test harness, CI) has two or more. No attached console at all
/// reports zero. Only the count-of-one case is a genuine double-click.
#[cfg(windows)]
pub fn owns_console() -> bool {
    extern "system" {
        fn GetConsoleProcessList(lpdwProcessList: *mut u32, dwProcessCount: u32) -> u32;
    }
    // We only need the count; a two-slot buffer is enough to distinguish
    // "just us" (1) from "us + a parent shell" (>= 2).
    let mut pids = [0u32; 2];
    let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    count == 1
}

/// Non-Windows platforms have no equivalent double-click-into-a-transient-console
/// idiom, so we always report "not owned" and keep vanilla's CWD behavior.
#[cfg(not(windows))]
pub fn owns_console() -> bool {
    false
}

/// Print a prompt and block until the user presses Enter. Called on every exit
/// path when we [`owns_console`], so a double-click launch doesn't vanish before
/// the user can read what happened — most importantly the EULA agreement notice.
pub fn pause_before_exit() {
    use std::io::{Read, Write};

    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\nPress Enter to exit . . . ");
    let _ = stdout.flush();
    // A single byte is enough; we don't care what was typed, only that the user
    // acknowledged. Reading to EOF (e.g. no stdin) returns immediately, which is
    // fine — there's nobody to wait for in that case.
    let _ = std::io::stdin().read(&mut [0u8]);
}
