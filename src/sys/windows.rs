//! Windows lock back-end using `LockFileEx` / `UnlockFileEx`.
//!
//! Windows byte-range locks are mandatory (enforced by the kernel) and
//! work correctly over SMB/CIFS and the Microsoft NFS client, so **both**
//! [`LockBackend`] variants map to `LockFileEx` here.  The `Fcntl` variant
//! is accepted for API uniformity and produces identical behaviour.
//!
//! We lock the entire file by specifying a byte range of 2⁶⁴ − 1 bytes.

use std::fs::File;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::{FALSE, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    LockFileEx, UnlockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
};
use windows_sys::Win32::System::IO::OVERLAPPED;

use crate::backend::LockBackend;
use crate::error::Result;
use crate::lock::{LockKind, LockMode};

/// Build the `dwFlags` bitmask for `LockFileEx`.
#[inline]
fn flags(kind: LockKind, mode: LockMode) -> u32 {
    let mut f: u32 = 0;
    if kind == LockKind::Exclusive {
        f |= LOCKFILE_EXCLUSIVE_LOCK;
    }
    if mode == LockMode::NonBlocking {
        f |= LOCKFILE_FAIL_IMMEDIATELY;
    }
    f
}

/// A zeroed `OVERLAPPED` pinning the lock range at file offset 0.
#[inline]
fn overlapped() -> OVERLAPPED {
    // SAFETY: OVERLAPPED is a C POD type; zero is a valid initial state.
    unsafe { std::mem::zeroed() }
}

pub(crate) fn lock(
    file: &File,
    kind: LockKind,
    mode: LockMode,
    _backend: LockBackend,
) -> Result<()> {
    // Both backends are identical on Windows: LockFileEx already handles
    // SMB/CIFS and the MS NFS client at the driver level.
    let handle = file.as_raw_handle() as HANDLE;
    let flags = flags(kind, mode);
    let mut ol = overlapped();

    // SAFETY: handle is valid; ol is properly initialised.
    let ok = unsafe { LockFileEx(handle, flags, 0, u32::MAX, u32::MAX, &mut ol) };

    if ok != FALSE {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

pub(crate) fn unlock(file: &File, _backend: LockBackend) -> Result<()> {
    let handle = file.as_raw_handle() as HANDLE;
    let mut ol = overlapped();

    // SAFETY: handle is valid; ol is properly initialised.
    let ok = unsafe { UnlockFileEx(handle, 0, u32::MAX, u32::MAX, &mut ol) };

    if ok != FALSE {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

pub(crate) fn upgrade(file: &File, mode: LockMode, backend: LockBackend) -> Result<()> {
    unlock(file, backend)?;
    lock(file, LockKind::Exclusive, mode, backend)
}

pub(crate) fn downgrade(file: &File, backend: LockBackend) -> Result<()> {
    // Windows permits a shared lock to overlap an exclusive lock when both are
    // created on the same handle. Unlocking once then removes the exclusive
    // lock and leaves the shared lock in place.
    lock(file, LockKind::Shared, LockMode::Blocking, backend)?;
    unlock(file, backend)
}
