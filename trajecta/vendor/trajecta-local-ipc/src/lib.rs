//! Safe blocking local IPC streams for Trajecta.
//!
//! Windows uses named pipes with remote clients rejected. Unix platforms use
//! Unix domain sockets. No TCP fallback exists.

use std::io::{self, Read, Write};
use std::time::Duration;

mod platform;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as transport;
#[cfg(windows)]
use windows as transport;

pub use platform::{
    ProcessIdentity, StandardHandleInheritanceGuard, available_memory_bytes,
    current_process_identity, force_terminate_process, process_identity, process_identity_matches,
    suppress_standard_handle_inheritance,
};

/// Blocking local-only listener.
pub struct LocalListener(transport::Listener);

impl LocalListener {
    /// Binds an absolute Unix socket path or a Windows `\\.\pipe\...` name.
    pub fn bind(endpoint: &str) -> io::Result<Self> {
        transport::Listener::bind(endpoint).map(Self)
    }

    /// Accepts one local client connection.
    pub fn accept(&mut self) -> io::Result<LocalStream> {
        self.0.accept().map(LocalStream)
    }
}

/// Blocking bidirectional local-only byte stream.
pub struct LocalStream(transport::Stream);

impl LocalStream {
    /// Connects to an existing endpoint within `timeout`.
    pub fn connect(endpoint: &str, timeout: Duration) -> io::Result<Self> {
        transport::Stream::connect(endpoint, timeout).map(Self)
    }
}

impl Read for LocalStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Write for LocalStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{LocalListener, LocalStream};

    fn endpoint() -> String {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        #[cfg(windows)]
        {
            format!(
                r"\\.\pipe\trajecta-local-ipc-test-{}-{nonce}",
                std::process::id()
            )
        }
        #[cfg(unix)]
        {
            std::env::temp_dir()
                .join(format!(
                    "trajecta-local-ipc-test-{}-{nonce}.sock",
                    std::process::id()
                ))
                .to_string_lossy()
                .into_owned()
        }
    }

    #[test]
    fn local_stream_roundtrip() {
        let endpoint = endpoint();
        let mut listener = LocalListener::bind(&endpoint).expect("bind");
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let server = thread::spawn(move || {
            ready_tx.send(()).expect("ready");
            let mut stream = listener.accept().expect("accept");
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).expect("read");
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").expect("write");
            stream.flush().expect("flush");
        });
        ready_rx.recv().expect("ready");
        let mut client = LocalStream::connect(&endpoint, Duration::from_secs(2)).expect("connect");
        client.write_all(b"ping").expect("write");
        client.flush().expect("flush");
        let mut response = [0_u8; 4];
        client.read_exact(&mut response).expect("read");
        assert_eq!(&response, b"pong");
        server.join().expect("join");
    }
}
