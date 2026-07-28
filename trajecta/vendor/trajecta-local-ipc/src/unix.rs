use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) struct Listener {
    inner: UnixListener,
    endpoint: PathBuf,
}

impl Listener {
    pub(crate) fn bind(endpoint: &str) -> io::Result<Self> {
        let endpoint = Path::new(endpoint);
        if !endpoint.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix socket endpoint must be absolute",
            ));
        }
        let inner = UnixListener::bind(endpoint)?;
        Ok(Self {
            inner,
            endpoint: endpoint.to_path_buf(),
        })
    }

    pub(crate) fn accept(&mut self) -> io::Result<Stream> {
        self.inner.accept().map(|(inner, _)| Stream { inner })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.endpoint);
    }
}

pub(crate) struct Stream {
    inner: UnixStream,
}

impl Stream {
    pub(crate) fn connect(endpoint: &str, timeout: Duration) -> io::Result<Self> {
        let endpoint = Path::new(endpoint);
        if !endpoint.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix socket endpoint must be absolute",
            ));
        }
        let started = Instant::now();
        loop {
            match UnixStream::connect(endpoint) {
                Ok(inner) => return Ok(Self { inner }),
                Err(error)
                    if started.elapsed() < timeout
                        && matches!(
                            error.kind(),
                            io::ErrorKind::NotFound
                                | io::ErrorKind::ConnectionRefused
                                | io::ErrorKind::WouldBlock
                        ) =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buffer)
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
