pub mod client;
pub mod codec;
pub mod messages;
pub mod server;
pub mod state;
pub mod wire;

pub use client::IpcClient;
pub use messages::*;
pub use server::build_pipe_security_attributes;
pub use server::IpcServer;

// ---- the pipe wiri serves on ----------------------------------------------------
//
// A pipe name is machine-global on Windows: `\\.\pipe\wiri_control` is the same
// pipe for every user logged in on the machine.  The second user could
// therefore not create its pipe (the name was taken), the single-instance
// probe in `main.rs` told it that "another wiri is already running", and its
// clients would have talked to the first user's daemon.  The name wiri uses is
// therefore the pipe of one user in one session:
// `\\.\pipe\wiri_control_<user>_<session>`.

/// The environment variable that says which pipe to use; it overrides the name
/// wiri works out for itself.  Tests use it, and so does a client that was
/// told where the daemon listens (niri exports `$NIRI_SOCKET` to its children).
pub const PIPE_ENV_VAR: &str = "WIRI_PIPE";

/// The pipe name without the part that makes it the pipe of one user in one
/// session.  The pipe wiri serves on is [`pipe_path`].
///
/// Only a name prefix, not a listener endpoint. The single-instance probe in
/// `main.rs`, the IPC server ([`pipe_has_listener`]) and clients all resolve
/// [`pipe_path`], including the user/session suffix or `WIRI_PIPE` override.
pub const PIPE_PATH: &str = r"\\.\pipe\wiri_control";

/// The pipe wiri serves on: `\\.\pipe\wiri_control_<user>_<session>`.
///
/// The user and the session are the ones this process runs as (`GetUserNameW`
/// and `ProcessIdToSessionId`), so two users logged in on the same machine
/// (fast user switching, an RDP session) never share a pipe, and a user logged
/// in twice gets one pipe per session.  `WIRI_PIPE` overrides the name: that is
/// what tests use, and what a client that was told where the daemon listens
/// uses.
pub fn pipe_path() -> String {
    let over = std::env::var(PIPE_ENV_VAR).ok();
    pipe_path_with(over.as_deref(), &current_user_name(), current_session_id())
}

/// The pipe name for a given override, user and session: the parts the unit
/// tests hand in themselves, so that the name can be checked without a Windows
/// session.
fn pipe_path_with(over: Option<&str>, user: &str, session: u32) -> String {
    match over.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => format!("{}_{}_{}", PIPE_PATH, sanitize_user_name(user), session),
    }
}

/// The part of a user name that goes into the pipe name: letters, digits, `-`,
/// `_` and `.` are kept, everything else becomes `_` (a pipe name may not
/// contain a backslash).
fn sanitize_user_name(user: &str) -> String {
    let cleaned: String = user
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "user".to_string()
    } else {
        cleaned
    }
}

/// The name of the user this process runs as (`John Doe`).  Empty when the
/// system does not say.
fn current_user_name() -> String {
    use windows::core::PWSTR;
    use windows::Win32::System::WindowsProgramming::GetUserNameW;

    // The first call asks how much room the name needs.
    let mut len: u32 = 0;
    unsafe {
        let _ = GetUserNameW(PWSTR::null(), &mut len);
    }
    if len == 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; len as usize];
    let ok = unsafe { GetUserNameW(PWSTR(buffer.as_mut_ptr()), &mut len) };
    if ok.is_err() {
        return String::new();
    }
    user_name_from_wide(&buffer[..(len as usize).min(buffer.len())])
}

/// The name in a `GetUserNameW` buffer.  The length it reports counts the
/// terminating NUL, which would otherwise become a `_` in the pipe name
/// (`wiri_control_ark__1` instead of the documented `wiri_control_ark_1`).
fn user_name_from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// The session this process runs in (1 is the console session, more for an RDP
/// session).  0 when the system does not say.
fn current_session_id() -> u32 {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::GetCurrentProcessId;

    let mut session: u32 = 0;
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    if ok.is_ok() {
        session
    } else {
        0
    }
}

/// Whether somebody is listening on the pipe: the single-instance probe (the
/// same one `main.rs` does, but on the pipe of this user and session).
pub fn pipe_has_listener(path: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::Pipes::WaitNamedPipeW;

    let wide: Vec<u16> = std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();
    unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), 1) }.as_bool()
}

/// Wakes the task that tells event-stream subscribers what changed (`IpcServer::run_state_task`).
static STATE_CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Something that event-stream clients are told about may have changed: a window came or
/// went, the focus moved, a workspace was switched, a config was loaded.  Costs two atomic
/// operations, takes no lock and may be called from any thread, with any lock held; the
/// work (describing the engine, finding what differs, writing to the clients) is done by
/// one task, once for a burst of changes, and only while somebody is subscribed.
pub fn notify_state_changed() {
    STATE_CHANGED.notify_one();
}

/// Calls [`notify_state_changed`] when it is dropped: put one at the top of a function that
/// changes the engine and has several ways out.
pub struct StateChangeGuard;

impl Drop for StateChangeGuard {
    fn drop(&mut self) {
        notify_state_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pipe_name_carries_the_user_and_the_session() {
        assert_eq!(
            pipe_path_with(None, "alice", 1),
            r"\\.\pipe\wiri_control_alice_1"
        );
        assert_eq!(
            pipe_path_with(None, "alice", 42),
            r"\\.\pipe\wiri_control_alice_42"
        );
    }

    #[test]
    fn the_terminating_nul_of_the_user_name_is_not_part_of_it() {
        let wide: Vec<u16> = "ark\0".encode_utf16().collect();
        assert_eq!(user_name_from_wide(&wide), "ark");
        assert_eq!(
            pipe_path_with(None, &user_name_from_wide(&wide), 1),
            r"\\.\pipe\wiri_control_ark_1"
        );
        let bare: Vec<u16> = "ark".encode_utf16().collect();
        assert_eq!(user_name_from_wide(&bare), "ark");
    }

    #[test]
    fn two_users_and_two_sessions_never_share_a_pipe() {
        let alice = pipe_path_with(None, "alice", 1);
        let bob = pipe_path_with(None, "bob", 1);
        let alice_in_another_session = pipe_path_with(None, "alice", 2);
        assert_ne!(alice, bob, "two users");
        assert_ne!(alice, alice_in_another_session, "two sessions");
    }

    #[test]
    fn a_user_name_that_is_not_plain_is_made_safe() {
        // A pipe name may not contain a backslash.
        assert_eq!(
            pipe_path_with(None, r"DOMAIN\John Doe", 3),
            r"\\.\pipe\wiri_control_DOMAIN_John_Doe_3"
        );
        assert_eq!(
            pipe_path_with(None, "jörg", 1),
            r"\\.\pipe\wiri_control_j_rg_1"
        );
        // A name that is empty (or all spaces) still gives a name.
        assert_eq!(pipe_path_with(None, "", 1), r"\\.\pipe\wiri_control_user_1");
    }

    #[test]
    fn the_environment_says_which_pipe_to_use() {
        assert_eq!(
            pipe_path_with(Some(r"\\.\pipe\wiri_test"), "alice", 1),
            r"\\.\pipe\wiri_test"
        );
        // An empty or blank override is no override.
        assert_eq!(
            pipe_path_with(Some(""), "alice", 1),
            r"\\.\pipe\wiri_control_alice_1"
        );
        assert_eq!(
            pipe_path_with(Some("   "), "alice", 1),
            r"\\.\pipe\wiri_control_alice_1"
        );
    }

    #[test]
    fn the_pipe_of_this_machine_names_this_user_and_this_session() {
        let path = pipe_path();
        assert!(path.starts_with(PIPE_PATH), "{}", path);
        assert!(
            path.ends_with(&format!("_{}", current_session_id())),
            "{}",
            path
        );
        // The name after `\\.\pipe\` is one piece: a pipe name may not contain
        // a backslash.
        let name = path.strip_prefix(r"\\.\pipe\").unwrap_or_default();
        assert!(!name.contains('\\'), "{}", path);
    }
}
