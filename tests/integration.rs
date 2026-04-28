//! Integration tests for filelocks.
//!
//! Tests are parameterised over [`LockBackend`] so scenarios run against every
//! backend supported by the current platform and filesystem.

use std::env;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{self, Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use filelocks::{Error, FileLock, LockBackend, LockKind, LockMode, LockOptions};
use tempfile::NamedTempFile;

// ── helpers ───────────────────────────────────────────────────────────────────

fn open_rw(path: &Path) -> std::fs::File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .expect("open failed")
}

/// Acquire a lock with the given backend (convenience wrapper).
fn lock_with(
    path: &Path,
    kind: LockKind,
    mode: LockMode,
    backend: LockBackend,
) -> filelocks::Result<FileLock> {
    LockOptions::new()
        .backend(backend)
        .lock(open_rw(path), kind, mode)
}

// Run a closure for every backend variant supported on this platform/filesystem.
fn for_each_backend(mut f: impl FnMut(LockBackend)) {
    for backend in [LockBackend::Flock, LockBackend::Fcntl] {
        if backend_supported_for_tests(backend) {
            f(backend);
        }
    }
}

fn backend_supported_for_tests(backend: LockBackend) -> bool {
    let tmp = NamedTempFile::new().unwrap();
    match lock_with(tmp.path(), LockKind::Shared, LockMode::NonBlocking, backend) {
        Ok(_guard) => true,
        Err(Error::Unsupported(_)) => false,
        Err(e) => panic!("[{backend:?}] support probe failed: {e}"),
    }
}

// Run a closure for backends that enforce per-fd lock isolation within the
// same process. Fcntl only participates when OFD locks are available; targets
// without OFD support return Error::Unsupported instead of using process-scoped
// POSIX locks.
fn for_each_backend_cross_fd(mut f: impl FnMut(LockBackend)) {
    f(LockBackend::Flock);
    if fcntl_has_cross_fd_isolation() {
        f(LockBackend::Fcntl);
    }
}

fn fcntl_has_cross_fd_isolation() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple",
        target_os = "illumos"
    )) && backend_supported_for_tests(LockBackend::Fcntl)
}

const CHILD_HELPER_ENV: &str = "FILELOCK_RS_CHILD_HELPER";
const CHILD_PATH_ENV: &str = "FILELOCK_RS_CHILD_PATH";
const CHILD_BACKEND_ENV: &str = "FILELOCK_RS_CHILD_BACKEND";
const CHILD_KIND_ENV: &str = "FILELOCK_RS_CHILD_KIND";
const CHILD_MODE_ENV: &str = "FILELOCK_RS_CHILD_MODE";
const CHILD_ACTION_ENV: &str = "FILELOCK_RS_CHILD_ACTION";

const CHILD_OK: i32 = 0;
const CHILD_WOULD_BLOCK: i32 = 11;
const CHILD_UNSUPPORTED: i32 = 12;
const CHILD_ERROR: i32 = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildOutcome {
    Ok,
    WouldBlock,
    Unsupported,
    Error(i32),
}

fn backend_name(backend: LockBackend) -> &'static str {
    match backend {
        LockBackend::Flock => "flock",
        LockBackend::Fcntl => "fcntl",
    }
}

fn kind_name(kind: LockKind) -> &'static str {
    match kind {
        LockKind::Shared => "shared",
        LockKind::Exclusive => "exclusive",
    }
}

fn mode_name(mode: LockMode) -> &'static str {
    match mode {
        LockMode::Blocking => "blocking",
        LockMode::NonBlocking => "nonblocking",
    }
}

fn parse_backend(value: &str) -> LockBackend {
    match value {
        "flock" => LockBackend::Flock,
        "fcntl" => LockBackend::Fcntl,
        other => panic!("unknown backend {other}"),
    }
}

fn parse_kind(value: &str) -> LockKind {
    match value {
        "shared" => LockKind::Shared,
        "exclusive" => LockKind::Exclusive,
        other => panic!("unknown lock kind {other}"),
    }
}

fn parse_mode(value: &str) -> LockMode {
    match value {
        "blocking" => LockMode::Blocking,
        "nonblocking" => LockMode::NonBlocking,
        other => panic!("unknown lock mode {other}"),
    }
}

fn child_command(
    path: &Path,
    backend: LockBackend,
    kind: LockKind,
    mode: LockMode,
    action: &'static str,
) -> Command {
    let mut cmd = Command::new(env::current_exe().unwrap());
    cmd.arg("child_lock_helper")
        .arg("--exact")
        .arg("--ignored")
        .env(CHILD_HELPER_ENV, "1")
        .env(CHILD_PATH_ENV, path.as_os_str())
        .env(CHILD_BACKEND_ENV, backend_name(backend))
        .env(CHILD_KIND_ENV, kind_name(kind))
        .env(CHILD_MODE_ENV, mode_name(mode))
        .env(CHILD_ACTION_ENV, action);
    cmd
}

fn child_try_lock(
    path: &Path,
    backend: LockBackend,
    kind: LockKind,
    mode: LockMode,
) -> ChildOutcome {
    let output = child_command(path, backend, kind, mode, "hold")
        .output()
        .expect("spawn child lock helper");
    if !output.status.success() && !output.stderr.is_empty() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
    child_outcome(output.status)
}

fn child_outcome(status: ExitStatus) -> ChildOutcome {
    match status.code() {
        Some(CHILD_OK) => ChildOutcome::Ok,
        Some(CHILD_WOULD_BLOCK) => ChildOutcome::WouldBlock,
        Some(CHILD_UNSUPPORTED) => ChildOutcome::Unsupported,
        Some(code) => ChildOutcome::Error(code),
        None => ChildOutcome::Error(-1),
    }
}

fn wait_child_with_timeout(child: &mut Child, timeout: Duration) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll child status") {
            return status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            panic!("child lock helper timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore]
fn child_lock_helper() {
    if env::var_os(CHILD_HELPER_ENV).is_none() {
        return;
    }

    let path = env::var_os(CHILD_PATH_ENV).expect("child path env");
    let backend = parse_backend(&env::var(CHILD_BACKEND_ENV).expect("child backend env"));
    let kind = parse_kind(&env::var(CHILD_KIND_ENV).expect("child kind env"));
    let mode = parse_mode(&env::var(CHILD_MODE_ENV).expect("child mode env"));
    let action = env::var(CHILD_ACTION_ENV).expect("child action env");

    match lock_with(Path::new(&path), kind, mode, backend) {
        Ok(mut guard) => {
            if action == "append" {
                guard.file_mut().seek(SeekFrom::End(0)).unwrap();
                guard.file_mut().write_all(b"x").unwrap();
            }
            process::exit(CHILD_OK);
        }
        Err(Error::WouldBlock) => process::exit(CHILD_WOULD_BLOCK),
        Err(Error::Unsupported(_)) => process::exit(CHILD_UNSUPPORTED),
        Err(e) => {
            eprintln!("child lock helper failed: {e}");
            process::exit(CHILD_ERROR);
        }
    }
}

// ── basic smoke tests — both backends ────────────────────────────────────────

#[test]
fn exclusive_blocking_lock_and_unlock() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let guard = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b)
            .unwrap_or_else(|e| panic!("[{b:?}] lock failed: {e}"));
        assert_eq!(guard.kind(), LockKind::Exclusive);
        assert_eq!(guard.backend(), b);
        drop(guard);
    });
}

#[test]
fn shared_blocking_lock_and_unlock() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let guard = lock_with(tmp.path(), LockKind::Shared, LockMode::Blocking, b)
            .unwrap_or_else(|e| panic!("[{b:?}] lock failed: {e}"));
        assert_eq!(guard.kind(), LockKind::Shared);
        drop(guard);
    });
}

#[test]
fn multiple_shared_locks_same_process() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let _g1 = lock_with(tmp.path(), LockKind::Shared, LockMode::Blocking, b)
            .unwrap_or_else(|e| panic!("[{b:?}] first shared lock: {e}"));
        let _g2 = lock_with(tmp.path(), LockKind::Shared, LockMode::Blocking, b)
            .unwrap_or_else(|e| panic!("[{b:?}] second shared lock: {e}"));
    });
}

#[test]
fn exclusive_non_blocking_fails_when_locked() {
    for_each_backend_cross_fd(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let _g = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();
        let res = lock_with(tmp.path(), LockKind::Exclusive, LockMode::NonBlocking, b);
        assert!(
            matches!(res, Err(Error::WouldBlock)),
            "[{b:?}] expected WouldBlock, got: {res:?}",
        );
    });
}

#[test]
fn shared_non_blocking_fails_when_exclusively_locked() {
    for_each_backend_cross_fd(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let _g = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();
        let res = lock_with(tmp.path(), LockKind::Shared, LockMode::NonBlocking, b);
        assert!(
            matches!(res, Err(Error::WouldBlock)),
            "[{b:?}] expected WouldBlock, got: {res:?}",
        );
    });
}

#[test]
fn unlock_explicit_returns_file() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let guard = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();
        let mut file = guard
            .unlock()
            .unwrap_or_else(|(_f, e)| panic!("[{b:?}] unlock: {e}"));

        file.write_all(b"hello").unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
    });
}

#[test]
fn into_file_does_not_panic() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let guard = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();
        let _file = guard.into_file();
    });
}

#[test]
fn cross_process_non_blocking_lock_reports_contention() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let _guard = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();

        let outcome = child_try_lock(tmp.path(), b, LockKind::Exclusive, LockMode::NonBlocking);
        assert_eq!(
            outcome,
            ChildOutcome::WouldBlock,
            "[{b:?}] child should observe cross-process contention",
        );
    });
}

#[test]
fn explicit_unlock_releases_lock_for_other_processes() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let guard = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();
        let _file = guard
            .unlock()
            .unwrap_or_else(|(_f, e)| panic!("[{b:?}] unlock: {e}"));

        let outcome = child_try_lock(tmp.path(), b, LockKind::Exclusive, LockMode::NonBlocking);
        assert_eq!(
            outcome,
            ChildOutcome::Ok,
            "[{b:?}] child should lock after explicit unlock",
        );
    });
}

#[test]
fn into_file_keeps_lock_until_file_is_dropped() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let guard = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();
        let file = guard.into_file();

        let blocked = child_try_lock(tmp.path(), b, LockKind::Exclusive, LockMode::NonBlocking);
        assert_eq!(
            blocked,
            ChildOutcome::WouldBlock,
            "[{b:?}] returned file should keep the lock alive",
        );

        drop(file);

        let acquired = child_try_lock(tmp.path(), b, LockKind::Exclusive, LockMode::NonBlocking);
        assert_eq!(
            acquired,
            ChildOutcome::Ok,
            "[{b:?}] child should lock after returned file is dropped",
        );
    });
}

#[test]
fn blocking_child_waits_until_parent_unlocks() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let guard = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();
        let mut command = child_command(
            tmp.path(),
            b,
            LockKind::Exclusive,
            LockMode::Blocking,
            "append",
        );
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn blocking child helper");

        std::thread::sleep(Duration::from_millis(150));
        assert!(
            child.try_wait().expect("poll child").is_none(),
            "[{b:?}] blocking child exited before parent released the lock",
        );

        drop(guard);
        let status = wait_child_with_timeout(&mut child, Duration::from_secs(5));
        assert_eq!(
            child_outcome(status),
            ChildOutcome::Ok,
            "[{b:?}] child should acquire after parent unlocks",
        );

        let mut file = open_rw(tmp.path());
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"x", "[{b:?}] child should append exactly once");
    });
}

// ── upgrade / downgrade — both backends ───────────────────────────────────────

#[test]
fn downgrade_exclusive_to_shared() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let mut g1 = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();
        g1.downgrade()
            .unwrap_or_else(|e| panic!("[{b:?}] downgrade: {e}"));
        assert_eq!(g1.kind(), LockKind::Shared);

        // A second shared lock should now succeed.
        let _g2 = lock_with(tmp.path(), LockKind::Shared, LockMode::NonBlocking, b)
            .unwrap_or_else(|e| panic!("[{b:?}] shared after downgrade: {e}"));
    });
}

#[test]
fn upgrade_shared_to_exclusive_non_blocking_fails_when_contested() {
    for_each_backend_cross_fd(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let mut g1 = lock_with(tmp.path(), LockKind::Shared, LockMode::Blocking, b).unwrap();
        let _g2 = lock_with(tmp.path(), LockKind::Shared, LockMode::Blocking, b).unwrap();

        let res = g1.upgrade(LockMode::NonBlocking);
        assert!(
            matches!(res, Err(Error::WouldBlock)),
            "[{b:?}] upgrade should fail while another shared lock is held: {res:?}",
        );
    });
}

// ── file I/O through the guard ────────────────────────────────────────────────

#[test]
fn can_write_and_read_through_guard() {
    for_each_backend(|b| {
        let tmp = NamedTempFile::new().unwrap();
        let mut g = lock_with(tmp.path(), LockKind::Exclusive, LockMode::Blocking, b).unwrap();

        g.file_mut().write_all(b"filelocks").unwrap();
        g.file_mut().seek(SeekFrom::Start(0)).unwrap();
        let mut buf = String::new();
        g.file_mut().read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "filelocks");
    });
}

// ── default (flock) API still works ──────────────────────────────────────────

#[test]
fn filelock_lock_uses_flock_backend_by_default() {
    let tmp = NamedTempFile::new().unwrap();
    let file = open_rw(tmp.path());
    let guard = FileLock::lock(file, LockKind::Exclusive, LockMode::Blocking).unwrap();
    assert_eq!(guard.backend(), LockBackend::Flock);
}

// ── LockOptions builder ───────────────────────────────────────────────────────

#[test]
fn lock_options_default_is_flock() {
    let tmp = NamedTempFile::new().unwrap();
    let guard = LockOptions::new()
        .lock(open_rw(tmp.path()), LockKind::Shared, LockMode::Blocking)
        .unwrap();
    assert_eq!(guard.backend(), LockBackend::Flock);
}

#[test]
fn lock_options_fcntl_backend() {
    if !backend_supported_for_tests(LockBackend::Fcntl) {
        return;
    }

    let tmp = NamedTempFile::new().unwrap();
    let guard = LockOptions::new()
        .backend(LockBackend::Fcntl)
        .lock(open_rw(tmp.path()), LockKind::Exclusive, LockMode::Blocking)
        .unwrap();
    assert_eq!(guard.backend(), LockBackend::Fcntl);
    assert_eq!(guard.kind(), LockKind::Exclusive);
}

// ── cross-backend isolation ───────────────────────────────────────────────────
//
// flock and fcntl locks are tracked independently by the kernel, so an
// exclusive flock lock does NOT block an fcntl lock attempt and vice versa.
// This is correct kernel behaviour — document it with a test rather than
// treating it as a bug.

// On Linux, flock(2) and fcntl advisory locks are tracked by entirely
// separate kernel subsystems and never interact. Keep this assertion scoped to
// Linux because other kernels document different interactions.
#[test]
#[cfg(target_os = "linux")]
fn flock_and_fcntl_locks_are_independent() {
    if !backend_supported_for_tests(LockBackend::Fcntl) {
        return;
    }

    let tmp = NamedTempFile::new().unwrap();

    // Hold an exclusive flock lock ...
    let _g_flock = lock_with(
        tmp.path(),
        LockKind::Exclusive,
        LockMode::Blocking,
        LockBackend::Flock,
    )
    .unwrap();

    // ... an exclusive fcntl lock on the same file must also succeed because
    // the kernel tracks flock and fcntl separately.
    let g_fcntl = lock_with(
        tmp.path(),
        LockKind::Exclusive,
        LockMode::NonBlocking,
        LockBackend::Fcntl,
    );
    assert!(
        g_fcntl.is_ok(),
        "fcntl lock should succeed independently of flock lock; got: {g_fcntl:?}",
    );
}

// ── threading ─────────────────────────────────────────────────────────────────

#[test]
fn lock_guard_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<FileLock>();
}

#[test]
fn lock_guard_is_sync() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<FileLock>();
}

#[test]
fn threaded_exclusive_lock_serialises_writes_flock() {
    threaded_exclusive_lock(LockBackend::Flock);
}

#[test]
fn threaded_exclusive_lock_serialises_writes_fcntl() {
    threaded_exclusive_lock(LockBackend::Fcntl);
}

fn threaded_exclusive_lock(backend: LockBackend) {
    use std::sync::Arc;

    if !backend_supported_for_tests(backend) {
        return;
    }

    let tmp = NamedTempFile::new().unwrap();
    let path = Arc::new(tmp.path().to_owned());
    let mut handles = Vec::new();

    for _ in 0..8 {
        let p = Arc::clone(&path);
        handles.push(std::thread::spawn(move || {
            let mut g = LockOptions::new()
                .backend(backend)
                .lock(open_rw(&p), LockKind::Exclusive, LockMode::Blocking)
                .unwrap();
            g.file_mut().seek(SeekFrom::End(0)).unwrap();
            g.file_mut().write_all(b"x").unwrap();
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    let mut file = open_rw(&path);
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).unwrap();
    assert_eq!(buf.len(), 8, "[{backend:?}] each thread should append once");
}
