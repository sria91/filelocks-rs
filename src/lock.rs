//! Public `FileLock` type and associated enumerations.

use std::fs::File;
use std::mem::ManuallyDrop;

use crate::backend::LockBackend;
use crate::error::Result;
use crate::sys;

// ── LockKind ────────────────────────────────────────────────────────────────

/// The kind of lock to acquire.
///
/// | Kind        | Multiple holders? |
/// |-------------|-------------------|
/// | `Shared`    | Yes               |
/// | `Exclusive` | No                |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockKind {
    /// A **shared** (read) lock.  
    /// Many processes may hold a shared lock on the same file simultaneously,
    /// but an exclusive lock cannot coexist with them.
    Shared,

    /// An **exclusive** (write) lock.  
    /// At most one process may hold an exclusive lock; no shared locks may
    /// coexist with it.
    Exclusive,
}

// ── LockMode ────────────────────────────────────────────────────────────────

/// Whether the locking call should block until the lock is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockMode {
    /// Block the calling thread until the lock can be acquired.
    Blocking,

    /// Return immediately.  If the lock cannot be acquired,
    /// [`Error::WouldBlock`](crate::Error::WouldBlock) is returned.
    NonBlocking,
}

// ── FileLock ──────────────────────────────────────────────────────────────────

/// An RAII guard that holds a file lock.
///
/// The lock is released automatically on drop.  Call [`FileLock::unlock`]
/// to get the inner [`File`] back, or [`FileLock::into_file`] to reclaim it
/// without an explicit OS unlock.
///
/// # Example
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use filelocks::{Error, FileLock, LockKind, LockMode};
///
/// # fn run() -> filelocks::Result<()> {
/// let file = OpenOptions::new()
///     .read(true).write(true).create(true)
///     .open("example.lock")
///     ?;
///
/// // Non-blocking exclusive lock using the default (flock) backend.
/// match FileLock::lock(file, LockKind::Exclusive, LockMode::NonBlocking) {
///     Ok(guard) => {
///         println!("lock acquired");
///         // guard is dropped here, releasing the lock
///         drop(guard);
///         Ok(())
///     }
///     Err(Error::WouldBlock) => {
///         println!("file is already locked – try again later");
///         Ok(())
///     }
///     Err(e) => Err(e),
/// }
/// # }
/// ```
#[derive(Debug)]
pub struct FileLock {
    pub(crate) file: ManuallyDrop<File>,
    pub(crate) kind: LockKind,
    pub(crate) backend: LockBackend,
    pub(crate) armed: bool,
}

impl FileLock {
    // ── pub(crate) constructor used by LockOptions ────────────────────────

    pub(crate) fn new(file: File, kind: LockKind, backend: LockBackend) -> Self {
        Self {
            file: ManuallyDrop::new(file),
            kind,
            backend,
            armed: true,
        }
    }

    // ── Public constructors ───────────────────────────────────────────────

    /// Acquire a lock on `file` using the default [`LockBackend::Flock`] backend.
    ///
    /// For files on NFS or CIFS/SMB mounts use [`LockOptions`](crate::LockOptions)
    /// with [`LockBackend::Fcntl`](crate::LockBackend::Fcntl) instead.
    ///
    /// # Errors
    ///
    /// * [`Error::WouldBlock`](crate::Error::WouldBlock) — when `mode` is
    ///   [`LockMode::NonBlocking`] and the lock is held by another lock holder.
    /// * [`Error::Io`](crate::Error::Io) — for any other OS-level failure.
    pub fn lock(file: File, kind: LockKind, mode: LockMode) -> Result<Self> {
        let backend = LockBackend::Flock;
        sys::lock(&file, kind, mode, backend)?;
        Ok(Self::new(file, kind, backend))
    }

    // ── Inspection ───────────────────────────────────────────────────────

    /// Returns the [`LockKind`] currently held by this guard.
    #[inline]
    pub fn kind(&self) -> LockKind {
        self.kind
    }

    /// Returns the [`LockBackend`] used by this guard.
    #[inline]
    pub fn backend(&self) -> LockBackend {
        self.backend
    }

    /// Borrow the underlying [`File`] without releasing the lock.
    #[inline]
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Mutably borrow the underlying [`File`] without releasing the lock.
    #[inline]
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    // ── Lock management ──────────────────────────────────────────────────

    /// Attempt to **upgrade** a shared lock to an exclusive lock.
    ///
    /// On success the guard's kind becomes [`LockKind::Exclusive`].
    ///
    /// # Errors
    ///
    /// * [`Error::WouldBlock`](crate::Error::WouldBlock) — when `mode` is
    ///   [`LockMode::NonBlocking`] and the upgrade cannot happen immediately.
    /// * [`Error::Unsupported`](crate::Error::Unsupported) — when the selected
    ///   backend is unavailable for this platform or filesystem.
    /// * [`Error::Io`](crate::Error::Io) — for any other OS-level failure.
    ///
    /// > **Note:** Whether an upgrade is atomic is determined by the underlying
    /// > operating system primitive. Some backends must release the shared lock
    /// > before attempting the exclusive lock, so callers that cannot tolerate a
    /// > failed or contended upgrade should prefer taking an exclusive lock up
    /// > front.
    pub fn upgrade(&mut self, mode: LockMode) -> Result<()> {
        if self.kind == LockKind::Exclusive {
            return Ok(());
        }

        sys::upgrade(self.file(), mode, self.backend)?;
        self.kind = LockKind::Exclusive;
        Ok(())
    }

    /// **Downgrade** an exclusive lock to a shared lock.
    ///
    /// On most Unix backends this is a direct lock conversion. On Windows the
    /// backend first layers a shared byte-range lock on the same handle, then
    /// removes the exclusive lock.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`](crate::Error::Io) on unexpected OS failure.
    pub fn downgrade(&mut self) -> Result<()> {
        if self.kind == LockKind::Shared {
            return Ok(());
        }

        sys::downgrade(self.file(), self.backend)?;
        self.kind = LockKind::Shared;
        Ok(())
    }

    /// Release the lock and return the inner [`File`].
    ///
    /// This is equivalent to dropping the guard but lets you reuse the file
    /// handle.  Even when an OS error occurs the `File` is returned so the
    /// caller can close it.
    ///
    /// # Errors
    ///
    /// Returns `Err((file, Error::Io(_)))` if the OS unlock call fails.
    pub fn unlock(mut self) -> std::result::Result<File, (File, crate::Error)> {
        let unlock_res = sys::unlock(&self.file, self.backend);
        self.armed = false;
        // SAFETY: `self.file` is initialized in `new` and only extracted once
        // after disarming Drop for this instance.
        let file = unsafe { ManuallyDrop::take(&mut self.file) };

        match unlock_res {
            Ok(()) => Ok(file),
            Err(e) => Err((file, e)),
        }
    }

    /// Consume the guard and return the inner [`File`] **without** explicitly
    /// unlocking it.
    ///
    /// The lock is released when the last file descriptor referring to the
    /// file is closed.  Prefer [`Self::unlock`] for deterministic release.
    pub fn into_file(mut self) -> File {
        self.armed = false;
        // SAFETY: `self.file` is initialized in `new` and only extracted once
        // after disarming Drop for this instance.
        unsafe { ManuallyDrop::take(&mut self.file) }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Best-effort; ignore errors in drop.
        if self.armed {
            let _ = sys::unlock(&self.file, self.backend);
            // SAFETY: `self.file` remains initialized while armed, and this is
            // the unique drop path when ownership was not extracted.
            unsafe { ManuallyDrop::drop(&mut self.file) };
        }
    }
}
