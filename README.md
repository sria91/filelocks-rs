# filelocks

> Platform-native advisory file locking for Rust — blocking and non-blocking,
> shared and exclusive, with an OFD/LockFileEx backend for network shares.

## Features

- **Shared (read) locks** — multiple holders allowed simultaneously
- **Exclusive (write) locks** — mutual exclusion among cooperating lock users
- **Blocking** — park the thread until the lock is free
- **Non-blocking** — return `Err(Error::WouldBlock)` immediately
- **Two backends** — `flock` for local Unix files, OFD/`LockFileEx` for network shares
- **Upgrade / downgrade** — change lock kind without re-opening the file
- **RAII guard** — lock released automatically on drop
- Unsafe code confined to the two `sys/` back-end modules

## Platform / backend matrix

| Backend | Linux / Android | Apple platforms | illumos | Other Unix | Windows |
|---------|-----------------|-----------------|---------|------------|---------|
| `Flock` | `flock(2)`      | `flock(2)`      | `flock(2)` | `flock(2)` | `LockFileEx` |
| `Fcntl` | OFD locks `F_OFD_SETLK(W)` | OFD locks `F_OFD_SETLK(W)` | OFD locks `F_OFD_SETLK(W)` | `Error::Unsupported` | `LockFileEx` |

Use `Fcntl` for files that may reside on NFS or CIFS/SMB mounts. On Unix
targets without open-file-description lock support, this crate returns
`Error::Unsupported` instead of falling back to POSIX record locks.

> ⚠ **Why no POSIX fallback?** POSIX locks are per-process. Closing *any* file
> descriptor to the same inode from the same process can release *all* POSIX
> locks that process holds on that file. That cannot faithfully model this
> crate's per-guard RAII semantics.

> 📌 **flock vs fcntl are independent:** the Linux kernel tracks them in separate
> tables.  An active `flock` lock does not prevent a `fcntl` lock from being
> acquired by a different file description (and vice versa).  Never mix the two
> backends across cooperating processes.

## Usage

```toml
# Cargo.toml
[dependencies]
filelocks = "0.1"
```

### Local files (default `flock` backend)

```rust
use std::fs::OpenOptions;
use filelocks::{FileLock, LockKind, LockMode, Error};

let file = OpenOptions::new()
    .read(true).write(true).create(true)
    .open("my.lock")?;

match FileLock::lock(file, LockKind::Exclusive, LockMode::NonBlocking) {
    Ok(guard) => { /* critical section — guard dropped = lock released */ }
    Err(Error::WouldBlock) => { /* retry or bail */ }
    Err(e) => return Err(e.into()),
}
```

### Network mounts — NFS / CIFS (`fcntl` backend)

```rust
use std::fs::OpenOptions;
use filelocks::{LockBackend, LockKind, LockMode, LockOptions};

let file = OpenOptions::new()
    .read(true).write(true).create(true)
    .open("/mnt/nfs/share/my.lock")?;

let guard = LockOptions::new()
    .backend(LockBackend::Fcntl)   // OFD locks on supported Unix, LockFileEx on Windows
    .lock(file, LockKind::Exclusive, LockMode::Blocking)?;

// guard released on drop
```

### Upgrade / downgrade

```rust
let mut guard = LockOptions::new()
    .backend(LockBackend::Fcntl)
    .lock(file, LockKind::Shared, LockMode::Blocking)?;

// promote to exclusive (non-blocking — may return WouldBlock)
guard.upgrade(LockMode::NonBlocking)?;

// demote back to shared
guard.downgrade()?;
```

## API overview

```
// Acquisition
FileLock::lock(file, kind, mode)           → Result<FileLock>  (Flock backend)
LockOptions::new().backend(b).lock(…)      → Result<FileLock>  (any backend)

// Inspection
guard.kind()     → LockKind
guard.backend()  → LockBackend
guard.file()     → &File
guard.file_mut() → &mut File

// Lock management
guard.upgrade(mode)   → Result<()>
guard.downgrade()     → Result<()>
guard.unlock()        → Result<File, (File, Error)>   (explicit unlock)
guard.into_file()     → File                           (no explicit unlock)
```

## Error handling

```rust
use filelocks::Error;

match result {
    Err(Error::WouldBlock) => { /* lock contended — retry or skip */ }
    Err(Error::Unsupported(reason)) => { /* backend unavailable on this target */ }
    Err(Error::Io(e))      => { /* OS-level failure */ }
    Ok(guard)              => { /* success */ }
}
```

## License

MIT OR Apache-2.0
