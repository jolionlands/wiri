pub mod client;
pub mod codec;
pub mod messages;
pub mod server;

pub use client::IpcClient;
pub use messages::*;
pub use server::build_pipe_security_attributes;
pub use server::IpcServer;

pub const PIPE_PATH: &str = r"\\.\pipe\wiri_control";
