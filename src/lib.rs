pub mod a11y;
pub mod backend;
pub mod config;
pub mod hooks;
pub mod input;
pub mod ipc;
pub mod layout;
pub mod logging;
pub mod overlay;
pub mod screenshot;
pub mod utils;
pub mod window;

/// The version of this build: the crate version, and the commit it was built
/// from when the build script found one (`WIRI_GIT_HASH`).
///
/// It is what the IPC `version` request answers with, so that a client can tell
/// which wiri it is talking to.
pub fn version_string() -> String {
    match option_env!("WIRI_GIT_HASH") {
        Some(hash) if !hash.is_empty() => {
            format!("{} (commit {})", env!("CARGO_PKG_VERSION"), hash)
        }
        _ => env!("CARGO_PKG_VERSION").to_string(),
    }
}
