//! Safe host process and memory probes used by the local daemon.

use std::io;

/// PID plus an operating-system process-start identity that rejects PID reuse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessIdentity {
    /// Platform process identifier.
    pub pid: u32,
    /// Stable creation-time token for this PID incarnation.
    pub start_token: String,
}

/// Restores the process standard-handle inheritance flags when dropped.
///
/// Windows handle inheritance is process-wide, so callers should keep this
/// guard alive only around the immediate background-process spawn. Unix
/// returns a no-op guard because close-on-exec descriptor handling already
/// prevents this Windows-specific pipe-retention failure.
#[must_use = "the guard must remain alive until the child process is spawned"]
pub struct StandardHandleInheritanceGuard {
    inner: imp::StandardHandleInheritanceGuard,
}

/// Temporarily prevents child processes from inheriting the current standard
/// input, output, and error handles.
///
/// This is required when a CLI captured through pipes launches a long-lived
/// detached child: otherwise the child can retain the caller's pipe handles
/// and prevent the caller from ever observing EOF.
pub fn suppress_standard_handle_inheritance() -> io::Result<StandardHandleInheritanceGuard> {
    imp::suppress_standard_handle_inheritance()
        .map(|inner| StandardHandleInheritanceGuard { inner })
}

impl Drop for StandardHandleInheritanceGuard {
    fn drop(&mut self) {
        self.inner.restore();
    }
}

/// Returns the calling process identity.
pub fn current_process_identity() -> io::Result<ProcessIdentity> {
    imp::current_process_identity()
}

/// Returns the live identity of `pid`, or `NotFound` when it no longer exists.
pub fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    imp::process_identity(pid)
}

/// Returns whether `identity` still denotes a live process.
pub fn process_identity_matches(identity: &ProcessIdentity) -> io::Result<bool> {
    imp::process_identity_matches(identity)
}

/// Immediately terminates the matching process, rejecting PID reuse.
pub fn force_terminate_process(identity: &ProcessIdentity) -> io::Result<()> {
    imp::force_terminate_process(identity)
}

/// Returns currently available physical memory in bytes.
pub fn available_memory_bytes() -> io::Result<u64> {
    imp::available_memory_bytes()
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::mem;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::Foundation::{
        FILETIME, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        STILL_ACTIVE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    };

    use super::ProcessIdentity;

    pub(super) struct StandardHandleInheritanceGuard {
        inherited: Vec<HANDLE>,
    }

    impl StandardHandleInheritanceGuard {
        pub(super) fn restore(&mut self) {
            for handle in self.inherited.drain(..) {
                // SAFETY: every handle was returned by `GetStdHandle`, was
                // valid when recorded, and remains owned by the process. A
                // best-effort restore is appropriate during Drop.
                unsafe {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
                }
            }
        }
    }

    pub(super) fn suppress_standard_handle_inheritance()
    -> io::Result<StandardHandleInheritanceGuard> {
        let mut guard = StandardHandleInheritanceGuard {
            inherited: Vec::with_capacity(3),
        };
        for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: `kind` is one of the three documented standard-handle
            // selectors and the function returns a process-owned handle.
            let handle = unsafe { GetStdHandle(kind) };
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                continue;
            }
            let mut flags = 0_u32;
            // SAFETY: `flags` is writable and `handle` was returned by
            // `GetStdHandle`.
            if unsafe { GetHandleInformation(handle, &mut flags) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if flags & HANDLE_FLAG_INHERIT == 0 {
                continue;
            }
            // SAFETY: only the inheritance bit is changed; ownership and
            // access rights remain unchanged.
            if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            guard.inherited.push(handle);
        }
        Ok(guard)
    }

    pub(super) fn current_process_identity() -> io::Result<ProcessIdentity> {
        // SAFETY: `GetCurrentProcess` returns a valid pseudo-handle owned by the
        // operating system and requiring no close.
        let handle = unsafe { GetCurrentProcess() };
        Ok(ProcessIdentity {
            pid: std::process::id(),
            start_token: process_start_token(handle)?,
        })
    }

    pub(super) fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
        let Some(handle) = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)? else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "process no longer exists",
            ));
        };
        Ok(ProcessIdentity {
            pid,
            start_token: process_start_token(handle.as_raw_handle().cast())?,
        })
    }

    pub(super) fn process_identity_matches(identity: &ProcessIdentity) -> io::Result<bool> {
        let Some(handle) = open_process(identity.pid, PROCESS_QUERY_LIMITED_INFORMATION)? else {
            return Ok(false);
        };
        let mut exit_code = 0_u32;
        // SAFETY: `handle` is a valid process handle and `exit_code` is writable.
        if unsafe { GetExitCodeProcess(handle.as_raw_handle().cast(), &mut exit_code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if exit_code != STILL_ACTIVE as u32 {
            return Ok(false);
        }
        Ok(process_start_token(handle.as_raw_handle().cast())? == identity.start_token)
    }

    pub(super) fn force_terminate_process(identity: &ProcessIdentity) -> io::Result<()> {
        let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE;
        let Some(handle) = open_process(identity.pid, access)? else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "process no longer exists",
            ));
        };
        if process_start_token(handle.as_raw_handle().cast())? != identity.start_token {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "process identity changed because the PID was reused",
            ));
        }
        // SAFETY: `handle` was opened with PROCESS_TERMINATE and its creation
        // token was revalidated immediately before this call.
        if unsafe { TerminateProcess(handle.as_raw_handle().cast(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn available_memory_bytes() -> io::Result<u64> {
        let mut status = MEMORYSTATUSEX {
            dwLength: u32::try_from(mem::size_of::<MEMORYSTATUSEX>()).map_err(io::Error::other)?,
            ..MEMORYSTATUSEX::default()
        };
        // SAFETY: `status` is correctly sized, initialized, and writable.
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(status.ullAvailPhys)
    }

    fn open_process(pid: u32, access: u32) -> io::Result<Option<OwnedHandle>> {
        // SAFETY: arguments contain no pointers and inheritance is disabled.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(87) | Some(1168)) {
                return Ok(None);
            }
            return Err(error);
        }
        // SAFETY: ownership of this fresh valid handle is transferred exactly
        // once to `OwnedHandle`, which closes it on drop.
        Ok(Some(unsafe { OwnedHandle::from_raw_handle(handle.cast()) }))
    }

    fn process_start_token(handle: windows_sys::Win32::Foundation::HANDLE) -> io::Result<String> {
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        // SAFETY: `handle` permits query-limited information and every FILETIME
        // pointer references initialized writable storage.
        if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let token = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        Ok(format!("{token:016x}"))
    }
}

#[cfg(unix)]
mod imp {
    use std::fs;
    use std::io;
    use std::process::Command;

    use super::ProcessIdentity;

    pub(super) struct StandardHandleInheritanceGuard;

    impl StandardHandleInheritanceGuard {
        pub(super) const fn restore(&mut self) {}
    }

    pub(super) const fn suppress_standard_handle_inheritance()
    -> io::Result<StandardHandleInheritanceGuard> {
        Ok(StandardHandleInheritanceGuard)
    }

    pub(super) fn current_process_identity() -> io::Result<ProcessIdentity> {
        let pid = std::process::id();
        process_identity(pid)
    }

    pub(super) fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
        Ok(ProcessIdentity {
            pid,
            start_token: start_token(pid)?,
        })
    }

    pub(super) fn process_identity_matches(identity: &ProcessIdentity) -> io::Result<bool> {
        match start_token(identity.pid) {
            Ok(token) => Ok(token == identity.start_token),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(super) fn force_terminate_process(identity: &ProcessIdentity) -> io::Result<()> {
        if !process_identity_matches(identity)? {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "process identity does not match",
            ));
        }
        let status = Command::new("kill")
            .args(["-KILL", &identity.pid.to_string()])
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!("kill exited with {status}")));
        }
        Ok(())
    }

    pub(super) fn available_memory_bytes() -> io::Result<u64> {
        let text = fs::read_to_string("/proc/meminfo")?;
        let kib = text
            .lines()
            .find_map(|line| {
                let mut fields = line.split_whitespace();
                (fields.next() == Some("MemAvailable:"))
                    .then(|| fields.next()?.parse::<u64>().ok())
                    .flatten()
            })
            .ok_or_else(|| io::Error::other("MemAvailable is missing"))?;
        kib.checked_mul(1024)
            .ok_or_else(|| io::Error::other("available memory overflow"))
    }

    fn start_token(pid: u32) -> io::Result<String> {
        let text = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let after_name = text
            .rsplit_once(") ")
            .map(|(_, value)| value)
            .ok_or_else(|| io::Error::other("invalid /proc process stat"))?;
        after_name
            .split_whitespace()
            .nth(19)
            .map(str::to_owned)
            .ok_or_else(|| io::Error::other("process start time is missing"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_identity_and_memory_probe_are_real() {
        let identity = current_process_identity().expect("identity");
        assert_eq!(identity.pid, std::process::id());
        assert_eq!(
            process_identity(identity.pid).expect("pid identity"),
            identity
        );
        assert!(process_identity_matches(&identity).expect("probe"));
        assert!(!identity.start_token.is_empty());
        assert!(available_memory_bytes().expect("memory") > 0);
    }
}
