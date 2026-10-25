pub mod client;
pub mod codec;
pub mod messages;
pub mod server;
pub mod state;

pub use client::IpcClient;
pub use messages::*;
pub use server::build_pipe_security_attributes;
pub use server::IpcServer;

pub const PIPE_PATH: &str = r"\\.\pipe\wiri_control";

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
