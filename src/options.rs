//! [`LockOptions`] — builder for fine-grained lock acquisition.

use std::fs::File;

use crate::backend::LockBackend;
use crate::error::Result;
use crate::lock::{FileLock, LockKind, LockMode};
use crate::sys;

/// Builder for acquiring a [`FileLock`] with non-default options.
///
/// The default options match the behaviour of [`FileLock::lock`]:
/// `Flock` backend, caller-supplied kind and mode.
///
/// # Example — NFS-safe exclusive lock
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use filelocks::{LockBackend, LockKind, LockMode, LockOptions};
///
/// # fn run() -> filelocks::Result<()> {
/// let file = OpenOptions::new()
///     .read(true).write(true).create(true)
///     .open("/mnt/nfs/share/my.lock")
///     ?;
///
/// let guard = LockOptions::new()
///     .backend(LockBackend::Fcntl)
///     .lock(file, LockKind::Exclusive, LockMode::Blocking)
///     ?;
/// # drop(guard);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct LockOptions {
    backend: LockBackend,
}

impl Default for LockOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl LockOptions {
    /// Create a new `LockOptions` with the default [`LockBackend::Flock`] backend.
    pub fn new() -> Self {
        Self {
            backend: LockBackend::default(),
        }
    }

    /// Select the OS primitive used to acquire the lock.
    ///
    /// Use [`LockBackend::Fcntl`] for files that may reside on NFS or CIFS
    /// network mounts. On Unix targets without OFD-lock support, that backend
    /// returns [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn backend(mut self, backend: LockBackend) -> Self {
        self.backend = backend;
        self
    }

    /// Acquire a lock on `file` using the configured options.
    ///
    /// # Errors
    ///
    /// * [`Error::WouldBlock`](crate::Error::WouldBlock) — when `mode` is
    ///   [`LockMode::NonBlocking`] and the lock is currently held.
    /// * [`Error::Unsupported`](crate::Error::Unsupported) — when the selected
    ///   backend is unavailable for this platform or filesystem.
    /// * [`Error::Io`](crate::Error::Io) — for any OS-level failure.
    pub fn lock(self, file: File, kind: LockKind, mode: LockMode) -> Result<FileLock> {
        sys::lock(&file, kind, mode, self.backend)?;
        Ok(FileLock::new(file, kind, self.backend))
    }
}
