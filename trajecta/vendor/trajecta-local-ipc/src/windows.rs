use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    WaitNamedPipeW,
};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;

pub(crate) struct Listener {
    endpoint: Vec<u16>,
    pending: Option<File>,
}

impl Listener {
    pub(crate) fn bind(endpoint: &str) -> io::Result<Self> {
        let endpoint = encode_endpoint(endpoint)?;
        let pending = create_server_pipe(&endpoint, true)?;
        Ok(Self {
            endpoint,
            pending: Some(pending),
        })
    }

    pub(crate) fn accept(&mut self) -> io::Result<Stream> {
        let file = self.pending.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "listener has no pending pipe")
        })?;
        // SAFETY: `file` owns a valid named-pipe server handle created by
        // `CreateNamedPipeW`; null OVERLAPPED requests a synchronous wait.
        let connected = unsafe { ConnectNamedPipe(file.as_raw_handle().cast(), ptr::null_mut()) };
        if connected == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                self.pending = create_server_pipe(&self.endpoint, false).ok();
                return Err(error);
            }
        }
        self.pending = Some(create_server_pipe(&self.endpoint, false)?);
        Ok(Stream {
            file,
            server_end: true,
        })
    }
}

pub(crate) struct Stream {
    file: File,
    server_end: bool,
}

impl Stream {
    pub(crate) fn connect(endpoint: &str, timeout: Duration) -> io::Result<Self> {
        let endpoint = encode_endpoint(endpoint)?;
        let started = Instant::now();
        loop {
            // SAFETY: `endpoint` is a NUL-terminated UTF-16 string and every
            // other pointer is null as permitted by `CreateFileW`.
            let handle = unsafe {
                CreateFileW(
                    endpoint.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                // SAFETY: ownership of this fresh valid handle is transferred
                // exactly once to `File`, which closes it on drop.
                let file = unsafe { File::from_raw_handle(handle.cast()) };
                return Ok(Self {
                    file,
                    server_end: false,
                });
            }

            let error = io::Error::last_os_error();
            let retryable = matches!(
                error.raw_os_error(),
                Some(code) if code == ERROR_PIPE_BUSY as i32 || code == ERROR_FILE_NOT_FOUND as i32
            );
            if !retryable || started.elapsed() >= timeout {
                return Err(error);
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            let wait_ms = remaining.as_millis().clamp(1, u128::from(u32::MAX)) as u32;
            // SAFETY: `endpoint` remains a valid NUL-terminated UTF-16 string
            // for the duration of this synchronous wait.
            let waited = unsafe { WaitNamedPipeW(endpoint.as_ptr(), wait_ms.min(50)) };
            if waited == 0 {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file.read(buffer)
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.file.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if self.server_end {
            // SAFETY: this is the still-owned server end of a named-pipe
            // instance. Flushing before disconnect guarantees the peer can
            // consume the complete framed response. Neither call transfers or
            // invalidates ownership; `File` closes the handle afterward.
            unsafe {
                FlushFileBuffers(self.file.as_raw_handle().cast());
                DisconnectNamedPipe(self.file.as_raw_handle().cast());
            }
        }
    }
}

fn create_server_pipe(endpoint: &[u16], first_instance: bool) -> io::Result<File> {
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first_instance {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: `endpoint` is a NUL-terminated UTF-16 string. The returned handle
    // is checked before ownership is transferred to `File`. Null security
    // attributes use the daemon process token's default DACL; remote clients
    // are independently rejected by `PIPE_REJECT_REMOTE_CLIENTS`.
    let handle = unsafe {
        CreateNamedPipeW(
            endpoint.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of this fresh valid handle is transferred exactly once
    // to `File`, which closes it on drop.
    Ok(unsafe { File::from_raw_handle(handle.cast()) })
}

fn encode_endpoint(endpoint: &str) -> io::Result<Vec<u16>> {
    if !endpoint.starts_with(r"\\.\pipe\") || endpoint.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            r"Windows endpoint must begin with \\.\pipe\ and contain no NUL",
        ));
    }
    Ok(OsStr::new(endpoint).encode_wide().chain(Some(0)).collect())
}
