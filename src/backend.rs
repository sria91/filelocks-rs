//! [`LockBackend`] — the OS primitive used to acquire a lock.

/// Which OS locking primitive is used when acquiring a [`crate::FileLock`].
///
/// | Backend | Unix mechanism                  | Windows      | Network filesystems |
/// |---------|---------------------------------|--------------|---------------------|
/// | `Flock` | `flock(2)`                      | `LockFileEx` | Local Unix only     |
/// | `Fcntl` | OFD locks where available (*)   | `LockFileEx` | Preferred backend   |
///
/// (*) The Unix `Fcntl` backend uses open-file-description locks on supported
/// targets such as Linux, Android, Apple platforms, and illumos. It returns
/// [`crate::Error::Unsupported`] on Unix targets where only process-scoped
/// POSIX record locks are available, because POSIX locks cannot faithfully
/// model this crate's per-guard RAII semantics.
///
/// On Windows, `LockFileEx` is used for both variants; the distinction exists
/// for cross-platform configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LockBackend {
    /// `flock(2)` on Unix, `LockFileEx` on Windows.
    ///
    /// Fast and suitable for **local** file systems.  
    /// **Not reliable over NFS or CIFS/SMB on Unix.**
    #[default]
    Flock,

    /// `fcntl(2)` OFD locks on supported Unix targets, and `LockFileEx` on
    /// Windows.
    ///
    /// Prefer this backend for files that may reside on a mounted network
    /// share. On Unix targets without OFD-lock support, acquiring this backend
    /// returns [`crate::Error::Unsupported`] instead of falling back to
    /// process-scoped POSIX locks.
    Fcntl,
}
