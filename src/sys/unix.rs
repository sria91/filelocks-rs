//! Unix lock back-ends: `flock(2)` and `fcntl(2)`.
//!
//! The module dispatches at runtime based on [`LockBackend`]:
//!
//! | Backend | Mechanism                        | NFS/CIFS? | Scope                |
//! |---------|----------------------------------|-----------|----------------------|
//! | `Flock` | `flock(2)`                       | ❌        | Per open-file-desc   |
//! | `Fcntl` | OFD locks on supported Unix      | ✅        | Per open-file-desc   |
//!
//! The `Fcntl` backend intentionally does not fall back to process-scoped POSIX
//! record locks. POSIX locks cannot faithfully implement per-guard RAII
//! semantics because closing any descriptor for the same file in the process can
//! release every POSIX lock held by that process.

use std::fs::File;
use std::os::unix::io::AsRawFd;

use crate::backend::LockBackend;
use crate::error::Result;
use crate::lock::{LockKind, LockMode};

// ── Public dispatch ──────────────────────────────────────────────────────────

pub(crate) fn lock(
    file: &File,
    kind: LockKind,
    mode: LockMode,
    backend: LockBackend,
) -> Result<()> {
    match backend {
        LockBackend::Flock => flock_backend::lock(file, kind, mode),
        LockBackend::Fcntl => fcntl_backend::lock(file, kind, mode),
    }
}

pub(crate) fn unlock(file: &File, backend: LockBackend) -> Result<()> {
    match backend {
        LockBackend::Flock => flock_backend::unlock(file),
        LockBackend::Fcntl => fcntl_backend::unlock(file),
    }
}

pub(crate) fn upgrade(file: &File, mode: LockMode, backend: LockBackend) -> Result<()> {
    lock(file, LockKind::Exclusive, mode, backend)
}

pub(crate) fn downgrade(file: &File, backend: LockBackend) -> Result<()> {
    lock(file, LockKind::Shared, LockMode::Blocking, backend)
}

// ── flock(2) ─────────────────────────────────────────────────────────────────

mod flock_backend {
    use super::*;
    use libc::{LOCK_EX, LOCK_NB, LOCK_SH, LOCK_UN};

    #[inline]
    fn op(kind: LockKind, mode: LockMode) -> libc::c_int {
        let base = match kind {
            LockKind::Shared => LOCK_SH,
            LockKind::Exclusive => LOCK_EX,
        };
        match mode {
            LockMode::Blocking => base,
            LockMode::NonBlocking => base | LOCK_NB,
        }
    }

    pub fn lock(file: &File, kind: LockKind, mode: LockMode) -> Result<()> {
        let fd = file.as_raw_fd();
        loop {
            // SAFETY: fd is valid for the lifetime of `file`.
            let ret = unsafe { libc::flock(fd, op(kind, mode)) };
            if ret == 0 {
                return Ok(());
            }

            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err.into());
        }
    }

    pub fn unlock(file: &File) -> Result<()> {
        let fd = file.as_raw_fd();
        loop {
            // SAFETY: fd is valid for the lifetime of `file`.
            let ret = unsafe { libc::flock(fd, LOCK_UN) };
            if ret == 0 {
                return Ok(());
            }

            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err.into());
        }
    }
}

// ── fcntl(2): OFD locks ──────────────────────────────────────────────────────

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "illumos"
))]
mod fcntl_backend {
    use super::*;

    // OFD lock command constants.
    // We define them ourselves for compatibility with older versions of the
    // `libc` crate that may not yet expose them.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    mod ofd {
        pub const F_OFD_SETLK: libc::c_int = 37;
        pub const F_OFD_SETLKW: libc::c_int = 38;
    }

    #[cfg(target_vendor = "apple")]
    mod ofd {
        pub const F_OFD_SETLK: libc::c_int = 90;
        pub const F_OFD_SETLKW: libc::c_int = 91;
    }

    #[cfg(target_os = "illumos")]
    mod ofd {
        pub const F_OFD_SETLK: libc::c_int = 48;
        pub const F_OFD_SETLKW: libc::c_int = 49;
    }

    const OFD_UNSUPPORTED: &str =
        "open-file-description locks are not supported by this OS version or filesystem";

    /// Build the fcntl command integer for acquiring a lock.
    #[inline]
    fn setlk_cmd(mode: LockMode) -> libc::c_int {
        match mode {
            LockMode::Blocking => ofd::F_OFD_SETLKW,
            LockMode::NonBlocking => ofd::F_OFD_SETLK,
        }
    }

    /// Build the fcntl command integer for unlocking.
    #[inline]
    fn unlock_cmd() -> libc::c_int {
        // For unlock, use the non-blocking variant (l_type = F_UNLCK never
        // blocks regardless of which setlk command is used).
        ofd::F_OFD_SETLK
    }

    /// Populate a zeroed `flock` struct for the given kind and range
    /// (always the entire file: start=0, len=0).
    ///
    /// # Safety
    ///
    /// `libc::flock` is a C POD type; a zero-initialized value is valid.
    #[inline]
    unsafe fn make_flock(l_type: libc::c_short) -> libc::flock {
        // SAFETY: caller guarantees it's valid to zero-init this type.
        let mut fl: libc::flock = std::mem::zeroed();
        fl.l_type = l_type;
        fl.l_whence = libc::SEEK_SET as libc::c_short;
        fl.l_start = 0;
        fl.l_len = 0; // 0 = "to end of file" (locks the whole file)
        fl.l_pid = 0; // required for OFD locks
        fl
    }

    /// Call `fcntl` and translate lock-specific errno values.
    ///
    /// EACCES and EAGAIN both indicate lock contention for `F_OFD_SETLK`.
    fn fcntl_call(
        fd: libc::c_int,
        cmd: libc::c_int,
        fl: &mut libc::flock,
        map_contention: bool,
    ) -> Result<()> {
        loop {
            // SAFETY: fd and fl are valid; cmd is a recognised fcntl lock command.
            let ret = unsafe { libc::fcntl(fd, cmd, fl as *mut libc::flock) };
            if ret != -1 {
                return Ok(());
            }

            let err = std::io::Error::last_os_error();
            let raw = err.raw_os_error().unwrap_or(0);
            if raw == libc::EINTR {
                continue;
            }
            if map_contention && (raw == libc::EACCES || raw == libc::EAGAIN) {
                return Err(crate::Error::WouldBlock);
            }
            if raw == libc::EINVAL || raw == libc::ENOTSUP || raw == libc::EOPNOTSUPP {
                return Err(crate::Error::Unsupported(OFD_UNSUPPORTED));
            }
            return Err(crate::Error::Io(err));
        }
    }

    pub fn lock(file: &File, kind: LockKind, mode: LockMode) -> Result<()> {
        let fd = file.as_raw_fd();
        let l_type = match kind {
            LockKind::Shared => libc::F_RDLCK as libc::c_short,
            LockKind::Exclusive => libc::F_WRLCK as libc::c_short,
        };
        // SAFETY: flock struct is valid to zero-init (see make_flock).
        let mut fl = unsafe { make_flock(l_type) };
        fcntl_call(fd, setlk_cmd(mode), &mut fl, true)
    }

    pub fn unlock(file: &File) -> Result<()> {
        let fd = file.as_raw_fd();
        // SAFETY: flock struct is valid to zero-init.
        let mut fl = unsafe { make_flock(libc::F_UNLCK as libc::c_short) };
        fcntl_call(fd, unlock_cmd(), &mut fl, false)
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "illumos"
)))]
mod fcntl_backend {
    use super::*;

    const UNSUPPORTED: &str = "open-file-description locks are unavailable on this Unix target";

    pub fn lock(_file: &File, _kind: LockKind, _mode: LockMode) -> Result<()> {
        Err(crate::Error::Unsupported(UNSUPPORTED))
    }

    pub fn unlock(_file: &File) -> Result<()> {
        Err(crate::Error::Unsupported(UNSUPPORTED))
    }
}
