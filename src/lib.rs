//! # filelocks
//!
//! Platform-native advisory file locking for Unix and Windows, including a
//! backend intended for NFS and CIFS/SMB network file systems.
//!
//! ## Backends
//!
//! | [`LockBackend`] | Unix mechanism                         | Windows      | Network filesystems |
//! |-----------------|----------------------------------------|--------------|---------------------|
//! | `Flock`         | `flock(2)`                             | `LockFileEx` | Local Unix only     |
//! | `Fcntl`         | OFD locks where supported              | `LockFileEx` | Preferred backend   |
//!
//! See [`LockBackend::Fcntl`] for platform support details. This crate does
//! not silently fall back to process-scoped POSIX locks on Unix targets where
//! OFD locks are unavailable.
//!
//! ## Quick start — local files
//!
//! ```no_run
//! use std::fs::OpenOptions;
//! use filelocks::{FileLock, LockKind, LockMode};
//!
//! # fn run() -> filelocks::Result<()> {
//! let file = OpenOptions::new()
//!     .read(true).write(true).create(true)
//!     .open("my.lock")
//!     ?;
//!
//! let lock = FileLock::lock(file, LockKind::Exclusive, LockMode::Blocking)?;
//! // lock released on drop
//! # drop(lock);
//! # Ok(())
//! # }
//! ```
//!
//! ## Quick start — NFS / CIFS network mounts
//!
//! ```no_run
//! use std::fs::OpenOptions;
//! use filelocks::{LockBackend, LockKind, LockMode, LockOptions};
//!
//! # fn run() -> filelocks::Result<()> {
//! let file = OpenOptions::new()
//!     .read(true).write(true).create(true)
//!     .open("/mnt/nfs/shared.lock")
//!     ?;
//!
//! // LockBackend::Fcntl uses OFD locks on supported Unix targets and
//! // LockFileEx on Windows. Unsupported Unix targets return Error::Unsupported.
//! let lock = LockOptions::new()
//!     .backend(LockBackend::Fcntl)
//!     .lock(file, LockKind::Exclusive, LockMode::Blocking)
//!     ?;
//! # drop(lock);
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![warn(clippy::all)]

mod backend;
mod error;
mod lock;
mod options;

#[cfg(unix)]
#[path = "sys/unix.rs"]
mod sys;

#[cfg(windows)]
#[path = "sys/windows.rs"]
mod sys;

#[cfg(not(any(unix, windows)))]
compile_error!("filelocks only supports Unix and Windows targets");

pub use backend::LockBackend;
pub use error::{Error, Result};
pub use lock::{FileLock, LockKind, LockMode};
pub use options::LockOptions;
