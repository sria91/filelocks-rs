//! Error type for filelocks.

use std::fmt;

/// The result type used throughout this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur when acquiring or releasing a file lock.
#[derive(Debug)]
pub enum Error {
    /// The lock could not be acquired without blocking (non-blocking mode).
    WouldBlock,

    /// The requested locking primitive is not supported on this platform,
    /// OS version, or filesystem.
    Unsupported(&'static str),

    /// An I/O error returned by the operating system.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WouldBlock => {
                write!(
                    f,
                    "lock would block: resource is already held by another lock holder"
                )
            }
            Error::Unsupported(reason) => write!(f, "locking backend is not supported: {reason}"),
            Error::Io(e) => write!(f, "I/O error during file locking: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        // Map EWOULDBLOCK / EAGAIN / ERROR_LOCK_VIOLATION to the dedicated variant
        // so callers don't need to inspect raw OS error codes.
        use std::io::ErrorKind;
        match e.kind() {
            ErrorKind::WouldBlock => Error::WouldBlock,
            _ => {
                // Windows: ERROR_LOCK_VIOLATION = 33
                #[cfg(windows)]
                if e.raw_os_error() == Some(33) {
                    return Error::WouldBlock;
                }
                Error::Io(e)
            }
        }
    }
}
