//! Unix socket listener setup (P3-13, plan D5): before the daemon can
//! serve RPC, its socket path must exist with the right permissions —
//! and a socket left behind by a previous run must be handled sanely.
//! [`setup_listener`] is the single entry point; it is **synchronous**
//! (plain `std` work plus one `tokio` conversion) so P3-17 can call it
//! during startup and then run the P3-16 async accept loop on the
//! returned `tokio::net::UnixListener`.
//!
//! Behavior (plan D5):
//! - create the parent directory (`create_dir_all`, best-effort; failure
//!   → [`SocketSetupError::DirCreate`] with a `--socket` hint — never a
//!   silent fallback to another path);
//! - if the socket file already exists, **probe** it with a
//!   `UnixStream::connect`: a live daemon →
//!   [`SocketSetupError::AlreadyRunning`]; a dead file → stale → remove
//!   ([`SocketSetupError::StaleRemoveFailed`] on failure) and rebind;
//! - bind, set the file mode to `0660` (a failure →
//!   [`SocketSetupError::Chmod`] — the documented client contract), and
//!   apply a **best-effort** group chown to `ramsleuth` (fallback
//!   `wheel`) — a chown failure only warns, it never blocks startup;
//! - re-apply the **per-user** POSIX ACLs on the socket from
//!   `/etc/ramsleuth/authorized-users` (the C21-01 state file) —
//!   best-effort and warn-only, and run on **every** socket creation
//!   because `/run` is a tmpfs that is wiped on reboot;
//! - set the socket non-blocking and hand it to the async side via
//!   `from_std` (which needs a runtime context — see [`setup_listener``]);
//!
//! Nothing in this module panics or exits: every failure path is a
//! `Result` error or a stderr warning.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use tokio::net::UnixListener as TokioUnixListener;

/// Startup failures for [`setup_listener`]. Every arm maps one concrete
/// failure to an operator-actionable [`std::fmt::Display`] message;
/// nothing in here panics or exits (plan D5).
///
/// `std::io::Error` is neither `Clone` nor `PartialEq`, so those two
/// traits are implemented by hand below (identity = variant + `kind` +
/// message text); `Debug` is derived. Group `chown` is deliberately not
/// an arm: it is best-effort and a failure only warns.
#[derive(Debug)]
pub enum SocketSetupError {
    /// The socket's parent directory could not be created (e.g.
    /// `/run/ramsleuth` without root).
    DirCreate(io::Error),
    /// The socket path already holds a **live** daemon (the probe
    /// connect succeeded).
    AlreadyRunning,
    /// A stale socket file was present but could not be removed.
    StaleRemoveFailed(io::Error),
    /// `bind(2)` on the socket path failed.
    Bind(io::Error),
    /// `chmod 0660` on the freshly bound socket failed.
    Chmod(io::Error),
}

/// `std::io::Error` is not `Clone`; preserve the observable identity
/// (`kind` + message text) instead.
fn clone_io_error(error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), error.to_string())
}

impl Clone for SocketSetupError {
    fn clone(&self) -> Self {
        match self {
            Self::DirCreate(e) => Self::DirCreate(clone_io_error(e)),
            Self::AlreadyRunning => Self::AlreadyRunning,
            Self::StaleRemoveFailed(e) => Self::StaleRemoveFailed(clone_io_error(e)),
            Self::Bind(e) => Self::Bind(clone_io_error(e)),
            Self::Chmod(e) => Self::Chmod(clone_io_error(e)),
        }
    }
}

/// `std::io::Error` is not `PartialEq`; compare the observable identity
/// (`kind` + message text) instead.
fn same_io_error(a: &io::Error, b: &io::Error) -> bool {
    a.kind() == b.kind() && a.to_string() == b.to_string()
}

impl PartialEq for SocketSetupError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AlreadyRunning, Self::AlreadyRunning) => true,
            (Self::DirCreate(a), Self::DirCreate(b))
            | (Self::StaleRemoveFailed(a), Self::StaleRemoveFailed(b))
            | (Self::Bind(a), Self::Bind(b))
            | (Self::Chmod(a), Self::Chmod(b)) => same_io_error(a, b),
            _ => false,
        }
    }
}

impl std::fmt::Display for SocketSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirCreate(e) => write!(
                f,
                "could not create the socket's parent directory ({e}); if you are unprivileged, override the path with `--socket /tmp/ramsleuth.sock`"
            ),
            Self::AlreadyRunning => write!(
                f,
                "a ramsleuth daemon is already running: the socket exists and accepts connections"
            ),
            Self::StaleRemoveFailed(e) => write!(
                f,
                "found a stale socket file but could not remove it ({e}); stop the old daemon or delete the file manually"
            ),
            Self::Bind(e) => write!(f, "could not bind the Unix socket listener ({e})"),
            Self::Chmod(e) => write!(f, "could not set the socket mode to 0660 ({e})"),
        }
    }
}

impl std::error::Error for SocketSetupError {}

/// Look up a group's gid via `getgrnam`; `None` when the group does not
/// exist.
fn group_gid(name: &str) -> Option<libc::gid_t> {
    let c_name = CString::new(name).ok()?;
    // SAFETY: `c_name` is a valid NUL-terminated byte string; `getgrnam`
    // returns either null (group absent) or a valid pointer to a
    // `passwd` entry that stays valid until the next `getgrnam` call.
    let entry = unsafe { libc::getgrnam(c_name.as_ptr()) };
    if entry.is_null() {
        None
    } else {
        // SAFETY: a non-null result is a valid `passwd` entry for the
        // duration of this call; we read only its `gr_gid` field.
        Some(unsafe { (*entry).gr_gid })
    }
}

/// Best-effort `chown(2)` of `path`'s **group** to `gid` (the owner is
/// left untouched via `-1`). Fails when the caller lacks the right
/// (e.g. a non-root dev run) — callers treat that as a warning, not an
/// error.
fn chown_group(path: &Path, gid: libc::gid_t) -> io::Result<()> {
    // A socket path containing an interior NUL cannot be passed to
    // `chown(2)`; treat it as a (non-fatal) failure like any other.
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket path contains NUL"))?;
    // SAFETY: `c_path` is NUL-terminated; `chown(2)` with the uid set
    // to `(uid_t::MAX)` (i.e. -1) leaves the current owner unchanged
    // and touches only the group.
    let no_owner = (-1i32) as libc::uid_t;
    let rc = unsafe { libc::chown(c_path.as_ptr(), no_owner, gid) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Apply the best-effort group ownership (plan D5): try the `ramsleuth`
/// group first, fall back to `wheel`; if neither exists, or the `chown`
/// fails (typical for a non-root dev run), log a **warning** and
/// continue — the `0660` mode already grants group access, and chown
/// must never block startup.
fn apply_socket_group(path: &Path) {
    let candidate = ["ramsleuth", "wheel"]
        .iter()
        .find_map(|name| group_gid(name).map(|gid| (*name, gid)));
    match candidate {
        Some((name, gid)) => {
            if let Err(e) = chown_group(path, gid) {
                eprintln!(
                    "ramsleuth-daemon: warning: could not chown the socket {} to group `{name}` (gid {gid}): {e}; keeping the current ownership (mode 0660 still applies)",
                    path.display()
                );
            }
        }
        None => eprintln!(
            "ramsleuth-daemon: warning: no `ramsleuth` or `wheel` group found; skipping the best-effort socket chown (mode 0660 still applies)"
        ),
    }
}

/// The C21-01 state file (frozen contract): one authorized username
/// per line (LF, no comments, no duplicates); dir `0755`, file `0644`.
const AUTHORIZED_USERS: &str = "/etc/ramsleuth/authorized-users";

/// Split the state file's content into candidate usernames: one per
/// line, trimmed, blank lines dropped, duplicates collapsed (the
/// C21-01 contract already forbids duplicates — this only defends
/// against a hand-edited file).
fn authorized_usernames(content: &str) -> Vec<String> {
    let mut users = Vec::new();
    for line in content.lines() {
        let user = line.trim();
        if !user.is_empty() && !users.iter().any(|u| u == user) {
            users.push(user.to_owned());
        }
    }
    users
}

/// Look up a user's uid via `getpwnam`; `None` when the user does not
/// exist (the same shape as [`group_gid`]).
fn user_uid(name: &str) -> Option<libc::uid_t> {
    let c_name = CString::new(name).ok()?;
    // SAFETY: `c_name` is a valid NUL-terminated byte string;
    // `getpwnam` returns either null (user absent) or a valid pointer
    // to a `passwd` entry that stays valid until the next `getpwnam`
    // call.
    let entry = unsafe { libc::getpwnam(c_name.as_ptr()) };
    if entry.is_null() {
        None
    } else {
        // SAFETY: a non-null result is a valid `passwd` entry for the
        // duration of this call; we read only its `pw_uid` field.
        Some(unsafe { (*entry).pw_uid })
    }
}

/// Probe whether `setfacl` is usable (the plan's `Command` probe):
/// `false` when it is absent from `PATH` or `--version` fails.
fn setfacl_available() -> bool {
    match std::process::Command::new("setfacl")
        .arg("--version")
        .output()
    {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

/// Best-effort `setfacl -m u:<uid>:rw` on `path` for one authorized
/// user. The numeric spec is resolved via [`user_uid`] so no
/// user-controlled string ever reaches the spawned command. `Err`
/// when the user is unknown, `setfacl` is absent, or `setfacl`
/// reports a nonzero status — the caller warns and continues.
fn apply_user_acl(path: &Path, user: &str) -> io::Result<()> {
    let uid = user_uid(user).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("user `{user}` is not in /etc/passwd"),
        )
    })?;
    // A socket path containing an interior NUL cannot be spawned;
    // `Command::output` reports it as a normal `Err` (warn-only).
    match std::process::Command::new("setfacl")
        .arg("-m")
        .arg(format!("u:{uid}:rw"))
        .arg(path)
        .output()
    {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(io::Error::other(format!(
            "setfacl exited with {status}: {stderr}",
            status = out.status,
            stderr = String::from_utf8_lossy(&out.stderr)
        ))),
        Err(e) => Err(e),
    }
}

/// Re-apply the per-user POSIX ACLs on the freshly bound socket at
/// `socket_path` from the state file at `state_path` (the
/// [`apply_socket_group`] precedent: best-effort, never blocks
/// startup). A missing state file only notes (the not-yet-seeded
/// fresh-install case — not an error); an unreadable state file, an
/// unavailable `setfacl`, an unknown user, or a failed `setfacl` run
/// only warns — the remaining users are still processed.
fn apply_authorized_user_acls(socket_path: &Path, state_path: &Path) {
    let content = match fs::read_to_string(state_path) {
        Ok(content) => content,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            eprintln!(
                "ramsleuth-daemon: note: no authorized-users state file at {} — no per-user socket ACLs to apply (mode 0660 + the group path are unaffected)",
                state_path.display()
            );
            return;
        }
        Err(e) => {
            eprintln!(
                "ramsleuth-daemon: warning: could not read the authorized-users state file {} ({e}) — skipping the per-user socket ACLs",
                state_path.display()
            );
            return;
        }
    };
    let users = authorized_usernames(&content);
    if users.is_empty() {
        return;
    }
    if !setfacl_available() {
        eprintln!(
            "ramsleuth-daemon: warning: `setfacl` is not available — skipping the per-user socket ACLs for {} authorized user(s) (mode 0660 + the group path still work)",
            users.len()
        );
        return;
    }
    for user in users {
        match apply_user_acl(socket_path, &user) {
            Ok(()) => eprintln!(
                "ramsleuth-daemon: granted per-user socket ACL u:rw on {} to `{user}` (from the authorized-users state file)",
                socket_path.display()
            ),
            Err(e) => eprintln!(
                "ramsleuth-daemon: warning: could not apply the per-user socket ACL for `{user}` ({e}) — continuing"
            ),
        }
    }
}

/// Prepare the daemon's Unix socket listener at `socket_path`.
///
/// Synchronous startup step (the async accept loop is P3-16/P3-17):
///
/// 1. create the parent directory (`create_dir_all`; failure →
///    [`SocketSetupError::DirCreate`] — never a silent path fallback);
/// 2. if the socket file exists, **probe** it with a `UnixStream`
///    connect: live daemon → [`SocketSetupError::AlreadyRunning`]; dead
///    file → stale → remove ([`SocketSetupError::StaleRemoveFailed`] on
///    failure) and rebind;
/// 3. bind a std [`UnixListener`] ([`SocketSetupError::Bind`] on
///    failure);
/// 4. set the file mode to `0660` ([`SocketSetupError::Chmod`] on
///    failure — the documented client contract);
/// 5. best-effort group chown (`ramsleuth` → `wheel`) plus a
///    best-effort re-apply of the per-user POSIX ACLs (step 5b, from
///    `/etc/ramsleuth/authorized-users`): a failure only warns (a
///    missing state file only notes), it never errors — the ACL pass
///    runs on every socket creation because `/run` is a tmpfs;
/// 6. set the socket non-blocking and convert it to a `tokio` listener
///    via `from_std` (a `from_std` conversion error maps to
///    [`SocketSetupError::Bind`]).
///
/// This function never panics or exits — every failure is a `Result`
/// error (or a stderr warning for the best-effort chown/ACL steps).
///
/// **Runtime context:** the final `from_std` step registers the socket
/// with the current tokio runtime's IO driver, so this function must be
/// called **from within a runtime context** (e.g. the P3-17
/// `#[tokio::main]` body, before the accept loop). Calling it before
/// any runtime exists makes tokio's `from_std` panic by design.
pub fn setup_listener(socket_path: &Path) -> Result<TokioUnixListener, SocketSetupError> {
    // (1) Parent directory: create it if missing. An empty parent (a
    // bare relative filename like `ramsleuth.sock`) needs no creation.
    if let Some(parent) = socket_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(SocketSetupError::DirCreate)?;
        }
    }

    // (2) Pre-existing socket file: probe before touching it. A
    // successful connect means another daemon owns the path (→
    // `AlreadyRunning`; the probe stream is dropped/closed as we return).
    // A failed connect means no live listener sits behind the file — it
    // is stale → remove it and rebind below.
    if socket_path.exists() {
        match UnixStream::connect(socket_path) {
            Ok(_probe) => return Err(SocketSetupError::AlreadyRunning),
            Err(_stale) => {
                fs::remove_file(socket_path).map_err(SocketSetupError::StaleRemoveFailed)?;
            }
        }
    }

    // (3) Bind the std listener (creates the socket fresh, or replaces
    // the stale file removed above).
    let std_listener = UnixListener::bind(socket_path).map_err(SocketSetupError::Bind)?;

    // (4) Mode 0660: group-writable for the owning group, world-off —
    // the access contract the clients rely on, so a failure here is a
    // hard error.
    let permissions = fs::Permissions::from_mode(0o660);
    fs::set_permissions(socket_path, permissions).map_err(SocketSetupError::Chmod)?;

    // (5) Best-effort group chown; a failure only warns (never an error).
    apply_socket_group(socket_path);

    // (5b) Best-effort per-user ACLs from the C21-01 state file; a
    // failure only warns (never an error) — `/run` is tmpfs, so this
    // runs on every socket creation.
    apply_authorized_user_acls(socket_path, Path::new(AUTHORIZED_USERS));

    // (6) Hand the bound socket to the async side. tokio's `from_std`
    // requires the socket to be non-blocking (its debug assert panics
    // otherwise — see tokio's own `from_std` example) and it registers
    // with the current runtime's IO driver, so the caller must be
    // inside a runtime context (the P3-17 `#[tokio::main]` body).
    std_listener
        .set_nonblocking(true)
        .map_err(SocketSetupError::Bind)?;
    TokioUnixListener::from_std(std_listener).map_err(SocketSetupError::Bind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::FileTypeExt;
    use std::path::PathBuf;

    /// Shared per-process temp root (the canonical shape:
    /// `temp_dir()/ramsleuth-test-<pid>`): unique by pid so parallel
    /// test binaries never collide. Each test uses its own subdirectory
    /// and removes it at the end, so parallel tests within this binary
    /// cannot collide either.
    fn test_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("ramsleuth-test-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create the shared test root");
        root
    }

    /// A per-test subdirectory (pre-created).
    fn test_dir(name: &str) -> PathBuf {
        let dir = test_root().join(name);
        fs::create_dir_all(&dir).expect("create the per-test directory");
        dir
    }

    /// Remove the test's subdirectory; then best-effort drop the shared
    /// root if it is now empty (`remove_dir` only succeeds on an empty
    /// directory, so parallel tests cannot race it away from each other).
    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
        if let Some(root) = dir.parent() {
            let _ = fs::remove_dir(root);
        }
    }

    /// (a) A fresh path: the missing parent is created, the bind
    /// succeeds, the socket file exists with mode `0660`, and the
    /// returned tokio listener is live (a `connect` to it succeeds).
    /// `#[tokio::test]` because `from_std` registers with the runtime's
    /// IO driver (plan D7: the `macros` feature is present for this).
    #[tokio::test]
    async fn fresh_path_binds_with_mode_0660() {
        let dir = test_root().join("fresh");
        let socket_path = dir.join("ramsleuth.sock");
        assert!(!dir.exists(), "precondition: no pre-existing parent dir");

        let listener = setup_listener(&socket_path).expect("a fresh path must set up cleanly");

        assert!(socket_path.exists(), "the socket file must exist after setup");
        let mode = fs::metadata(&socket_path)
            .expect("the socket file must be stat-able")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o660, "the socket mode must be 0660");
        // The listener must be live: the kernel accepts a `connect` to
        // the bound path (it lands in the accept backlog).
        let _probe =
            UnixStream::connect(&socket_path).expect("the returned listener must be live");

        drop(listener);
        cleanup(&dir);
    }

    /// (b) A stale socket file (present, nothing listening behind it)
    /// is removed and the path is rebound.
    #[tokio::test]
    async fn stale_socket_file_is_removed_and_rebound() {
        let dir = test_dir("stale");
        let socket_path = dir.join("ramsleuth.sock");
        fs::File::create(&socket_path).expect("plant the stale file");
        assert!(
            !fs::metadata(&socket_path).unwrap().file_type().is_socket(),
            "precondition: the planted file is not a socket"
        );

        let listener = setup_listener(&socket_path)
            .expect("a stale file must be removed and rebound");
        assert!(
            fs::metadata(&socket_path).unwrap().file_type().is_socket(),
            "the rebound path must hold a real socket"
        );

        drop(listener);
        cleanup(&dir);
    }

    /// (c) A live listener already owns the path: setup must report
    /// `AlreadyRunning` without touching the running daemon's socket.
    #[tokio::test]
    async fn live_listener_is_reported_as_already_running() {
        let dir = test_dir("live");
        let socket_path = dir.join("ramsleuth.sock");
        let live = UnixListener::bind(&socket_path).expect("bind the live listener");

        let err = setup_listener(&socket_path).expect_err("a live daemon must be detected");
        assert_eq!(err, SocketSetupError::AlreadyRunning);
        assert!(
            err.to_string().contains("already running"),
            "the error text must be operator-friendly: {err}"
        );

        drop(live);
        cleanup(&dir);
    }

    /// (d) An uncreatable parent (a regular file blocking the
    /// directory) yields `DirCreate` with the `--socket` hint — no
    /// silent fallback to another path.
    #[tokio::test]
    async fn uncreatable_parent_yields_dircreate_with_socket_hint() {
        let dir = test_dir("blocked");
        let blocker = dir.join("not-a-dir");
        fs::write(&blocker, b"blocker").expect("plant the blocker file");
        let socket_path = blocker.join("ramsleuth.sock");

        let err = setup_listener(&socket_path).expect_err("parent creation must fail");
        assert!(matches!(err, SocketSetupError::DirCreate(_)));
        assert!(
            err.to_string().contains("--socket"),
            "the DirCreate error must carry the --socket hint: {err}"
        );

        cleanup(&dir);
    }

    /// (e) The state-file parser: one username per line, stray
    /// whitespace trimmed, blank lines dropped, duplicates collapsed
    /// (first occurrence wins).
    #[test]
    fn authorized_usernames_parsing() {
        assert_eq!(
            authorized_usernames("alice\n\n  bob  \nalice\n\ncarol\n"),
            vec!["alice".to_string(), "bob".to_string(), "carol".to_string()]
        );
        assert_eq!(authorized_usernames(""), Vec::<String>::new());
        assert_eq!(authorized_usernames("\n   \n\t\n"), Vec::<String>::new());
    }

    /// (f) The best-effort ACL step degrades cleanly, never panicking:
    /// a missing state file is a no-op (the not-yet-seeded
    /// fresh-install case) and an unknown user only warns — `setfacl`
    /// is never spawned for one, so this holds with or without the
    /// `acl` package installed (the plan's CI case).
    #[test]
    fn acl_step_degrades_without_panicking() {
        let dir = test_dir("acl-degrade");
        let socket_path = dir.join("amsleuth.sock");
        fs::File::create(&socket_path).expect("plant the stand-in socket file");

        apply_authorized_user_acls(&socket_path, &dir.join("no-such-state-file"));

        let state = dir.join("authorized-users");
        fs::write(&state, "no-such-user-c21-03\n").expect("plant the state file");
        apply_authorized_user_acls(&socket_path, &state);

        assert!(
            socket_path.exists(),
            "the socket file must survive the best-effort step"
        );
        cleanup(&dir);
    }
}
