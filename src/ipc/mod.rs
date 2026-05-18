pub mod messages;
pub mod codec;
pub mod server;
pub mod client;

pub use messages::*;
pub use server::IpcServer;
pub use server::build_pipe_security_attributes;
pub use client::IpcClient;

pub const PIPE_PATH: &str = r"\\.\pipe\wiri_control";
