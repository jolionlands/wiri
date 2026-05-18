# IPC Module Specification

## Overview

The IPC module (src/ipc/) provides named pipe-based inter-process communication for external control of wiri, a Windows tiling window manager.

## Module Structure

`
ipc/
├── mod.rs           # Module exports, types, errors
├── server.rs       # IPC server implementation
├── client.rs       # IPC client implementation
├── messages.rs     # IpcMessage and IpcEvent definitions
└── codec.rs        # JSON serialization/deserialization
`

## Pipe Communication

### Pipe Path

`
\\.\pipe\wiri_control
`

The pipe name is fixed for simplicity, allowing external tools to connect without configuration.

### Protocol

- Windows named pipes
- JSON messaging
- Synchronous request/response pattern
- Async event streaming via subscriptions
- Tokio for async I/O operations

### Security Model

Pipe access is secured via SECURITY_ATTRIBUTES with a properly configured SACL:

- **Authentication Level**: RPC_C_AUTHN_LEVEL_PKT_PRIVACY (encrypted)
- **Impersonation Level**: SecurityImpersonation (server can impersonate client)
- **ACL**: Owner-only access (D:PAI(A;;FA;;;SY)(A;;FA;;;BA)) - only SYSTEM and Built-in Administrators can access
- **Inheritance**: No inheritance (InheritHandle: false)

## Message Format

### Request Structure

`json
{
  ""command"": ""invoke"",
  ""method"": ""method_name"",
  ""params"": { ... }
}
`

### Response Structure

Success:
`json
{
  ""success"": true,
  ""result"": { ... }
}
`

Error:
`json
{
  ""success"": false,
  ""error"": ""error message""
}
`

### Event Structure

`json
{
  ""event"": ""EventName"",
  ""data"": { ... }
}
`

## IpcMessage Enum

`ust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = ""command"", content = ""params"")]
pub enum IpcMessage {
    /// Request to tile a window at specified position
    TileRequest {
        window_hwnd: isize,
        position: (i32, i32),
        size: (u32, u32),
    },

    /// Move an existing window to new position
    WindowMove {
        window_hwnd: isize,
        x: i32,
        y: i32,
    },

    /// Resize an existing window
    WindowResize {
        window_hwnd: isize,
        width: u32,
        height: u32,
    },

    /// Get current window manager state
    GetState,

    /// Subscribe to event stream
    SubscribeEvents {
        events: Vec<IpcEvent>,
    },

    /// Get list of all managed windows
    WindowList,

    /// Unsubscribe from events
    UnsubscribeEvents {
        subscription_id: u64,
    },
}
`

## IpcEvent Enum

`ust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = ""event"")]
pub enum IpcEvent {
    /// Workspace configuration changed
    WorkspacesChanged {
        workspaces: Vec<WorkspaceInfo>,
    },

    /// New window opened and tiled
    WindowOpened {
        window_hwnd: isize,
        title: String,
        class_name: String,
        workspace_id: u32,
    },

    /// Window closed
    WindowClosed {
        window_hwnd: isize,
    },

    /// Workspace focus changed
    WorkspaceActivated {
        workspace_id: u32,
    },

    /// Overview/launcher opened
    OverviewOpened,

    /// Overview/launcher closed
    OverviewClosed,

    /// Configuration reloaded
    ConfigLoaded,

    /// Window focus changed
    WindowFocused {
        window_hwnd: isize,
    },

    /// Monitor configuration changed
    MonitorChanged {
        monitor_name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: u32,
    pub name: String,
    pub window_count: usize,
}
`

## IpcServer Struct

`ust
/// IPC server for external tool communication
pub struct IpcServer {
    /// Server handle
    pipe_server: NamedPipeServer,

    /// Tokio runtime for async operations
    rt: Handle,

    /// Client subscriptions for events
    subscriptions: Arc<RwLock<HashMap<u64, ClientSubscription>>>,

    /// Next subscription ID
    next_sub_id: AtomicU64,

    /// Shutdown signal
    shutdown: broadcast::Sender<()>,
}

struct NamedPipeServer {
    pipe_name: String,
    server: OverlappedOwnedHandle,
}

struct ClientSubscription {
    /// Client connection handle
    pipe: NamedPipeClient,

    /// Subscribed events filter
    events: HashSet<String>,

    /// Sender for disconnect notification
    close_tx: watch::Sender<bool>,
}

impl IpcServer {
    /// Create new IPC server
    pub fn new(rt: Handle) -> Result<Self> {
        let pipe_name = r""\\.\pipe\wiri_control"".to_string();

        // Security attributes for secure pipe creation
        let sa = SecurityAttributes::new()
            .with_authentication_level(RPC_C_AUTHN_LEVEL_PKT_PRIVACY)
            .with_impersonation_level(SecurityImpersonation)
            .with_acl(vec![
                // SYSTEM full access
                AccessAllowed {
                    sid: WellKnownSid::LocalSystem,
                    access: FILE_ALL_ACCESS,
                },
                // Built-in Administrators full access
                AccessAllowed {
                    sid: WellKnownSid::BuiltinAdministrators,
                    access: FILE_ALL_ACCESS,
                },
            ])
            .build();

        let pipe_server = NamedPipeServer::new(&pipe_name, &sa)?;

        Ok(Self {
            pipe_server,
            rt,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            next_sub_id: AtomicU64::new(1),
            shutdown: broadcast::new(1),
        })
    }

    /// Start accepting connections
    pub async fn run(&self) -> Result<()> {
        loop {
            tokio::select! {
                // Accept new client connections
                client = self.pipe_server.accept() => {
                    let client = client?;
                    self.handle_client(client).await;
                }
                // Handle shutdown
                _ = self.shutdown.subscribe().recv() => {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Handle incoming client connection
    async fn handle_client(&self, mut client: NamedPipeClient) {
        let subscription_id = self.next_sub_id.fetch_add(1, Ordering::SeqCst);

        let (close_tx, close_rx) = watch::channel(false);

        // Register subscription
        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(subscription_id, ClientSubscription {
                events: HashSet::new(),
                close_tx,
            });
        }

        // Spawn reading task
        let subs_clone = self.subscriptions.clone();
        let sub_id = subscription_id;
        let rt = self.rt.clone();

        rt.spawn(async move {
            Self::client_reader_loop(&mut client, sub_id, subs_clone, close_rx).await;
        });
    }

    /// Broadcast event to all subscribed clients
    pub async fn broadcast_event(&self, event: &IpcEvent) {
        let event_name = serde_json::to_string(event).unwrap_or_default();
        let mut subs = self.subscriptions.write().await;

        for (id, sub) in subs.iter_mut() {
            if sub.events.contains(&event_name) || sub.events.contains(""*"") {
                let data = serde_json::to_vec(event).unwrap_or_default();
                if let Err(e) = sub.pipe.write_all(&data).await {
                    // Client disconnected, mark for cleanup
                    let _ = sub.close_tx.send(true);
                }
            }
        }
    }

    /// Shutdown the server
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(());
    }
}
`

## IpcClient Struct

`ust
/// IPC client for external tool communication
pub struct IpcClient {
    /// Client connection handle
    pipe: NamedPipeClient,

    /// Tokio runtime handle
    rt: Handle,

    /// Subscription ID if subscribed to events
    subscription_id: Option<u64>,

    /// Event receiver for subscribed events
    event_rx: Option<UnboundedReceiver<IpcEvent>>,
}

impl IpcClient {
    /// Connect to the IPC server
    pub async fn connect() -> Result<Self> {
        let pipe_name = r""\\.\pipe\wiri_control"";

        // Wait for pipe to be available with timeout
        let timeout = 5000; // 5 seconds
        if !WaitNamedPipeW(pipe_name, timeout) {
            return Err(IpcError::PipeNotAvailable);
        }

        // Open existing pipe
        let pipe = NamedPipeClient::connect(pipe_name).await?;

        Ok(Self {
            pipe,
            rt: Handle::current(),
            subscription_id: None,
            event_rx: None,
        })
    }

    /// Send a message and wait for response
    pub async fn send(&mut self, message: &IpcMessage) -> Result<IpcResponse> {
        let data = serde_json::to_vec(message)?;
        self.pipe.write_all(&data).await?;

        // Read response length prefix (4 bytes)
        let mut len_buf = [0u8; 4];
        self.pipe.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        // Read response data
        let mut response_buf = vec![0u8; len];
        self.pipe.read_exact(&mut response_buf).await?;

        let response: IpcResponse = serde_json::from_slice(&response_buf)?;
        Ok(response)
    }

    /// Subscribe to events
    pub async fn subscribe(&mut self, events: Vec<IpcEvent>) -> Result<u64> {
        let message = IpcMessage::SubscribeEvents {
            events: events.clone(),
        };

        let response = self.send(&message).await?;

        if !response.success {
            return Err(IpcError::SubscriptionFailed(response.error));
        }

        let subscription_id: u64 = serde_json::from_value(response.result?)?;

        // Spawn event listener task
        let (tx, rx) = unbounded_channel();
        self.event_rx = Some(rx);
        self.subscription_id = Some(subscription_id);

        // Event loop continues in background
        // Events received via event_rx channel

        Ok(subscription_id)
    }

    /// Receive next event (requires active subscription)
    pub async fn next_event(&mut self) -> Result<IpcEvent> {
        self.event_rx
            .as_mut()
            .ok_or(IpcError::NotSubscribed)?
            .recv()
            .await
            .map_err(|_| IpcError::ConnectionLost)
    }

    /// Close the client connection
    pub fn close(&mut self) {
        // Signal event loop to exit
        self.event_rx = None;
        self.subscription_id = None;
    }
}
`

## Error Handling

`ust
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error(""pipe creation failed: {0}"")]
    PipeCreationFailed(String),

    #[error(""pipe not available"")]
    PipeNotAvailable,

    #[error(""connection failed: {0}"")]
    ConnectionFailed(String),

    #[error(""connection lost"")]
    ConnectionLost,

    #[error(""read error: {0}"")]
    ReadError(String),

    #[error(""write error: {0}"")]
    WriteError(String),

    #[error(""serialization error: {0}"")]
    SerializationError(#[from] serde_json::Error),

    #[error(""subscription failed: {0}"")]
    SubscriptionFailed(String),

    #[error(""not subscribed to events"")]
    NotSubscribed,

    #[error(""timeout"")]
    Timeout,

    #[error(""server error: {0}"")]
    ServerError(String),
}

impl From<io::Error> for IpcError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => IpcError::PipeNotAvailable,
            io::ErrorKind::TimedOut => IpcError::Timeout,
            io::ErrorKind::BrokenPipe => IpcError::ConnectionLost,
            _ => IpcError::ConnectionFailed(e.to_string()),
        }
    }
}
`

## Security Attributes Builder

`ust
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HANDLE, SECURITY_ATTRIBUTES, BOOL};
use windows::Win32::Security::{
    SECURITY_DESCRIPTOR, ACL, SID_AND_ATTRIBUTES,
    SECURITY_IMPERSONATION_LEVEL, TokenUser,
    GetTokenInformation, LookupAccountSidW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcessToken,
};

/// Builder for secure Windows pipe security attributes
pub struct SecurityAttributesBuilder {
    authentication_level: u32,
    impersonation_level: u32,
    ace_entries: Vec<AceEntry>,
}

struct AceEntry {
    sid: Vec<u8>,
    access_mask: u32,
}

impl SecurityAttributesBuilder {
    pub fn new() -> Self {
        Self {
            authentication_level: RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
            impersonation_level: SecurityImpersonation,
            ace_entries: Vec::new(),
        }
    }

    pub fn with_authentication_level(mut self, level: u32) -> Self {
        self.authentication_level = level;
        self
    }

    pub fn with_impersonation_level(mut self, level: u32) -> Self {
        self.impersonation_level = level;
        self
    }

    pub fn with_acl(mut self, aces: Vec<AceEntry>) -> Self {
        self.ace_entries = aces;
        self
    }

    /// Build SECURITY_ATTRIBUTES for CreateNamedPipeW
    pub fn build(&self) -> SECURITY_ATTRIBUTES {
        // Create security descriptor
        let mut sd = SECURITY_DESCRIPTOR::default();

        // Build DACL from ACE entries
        let dacl = self.build_dacl();

        unsafe {
            // Set DACL on security descriptor
            winapi::call!(
                ""windows::Win32::Security::Security::SetSecurityDescriptorDacl"",
                fn(
                    pSecurityDescriptor: *mut SECURITY_DESCRIPTOR,
                    bDaclPresent: BOOL,
                    pDacl: *mut ACL,
                    bDaclDefaulted: BOOL
                ) -> BOOL
            )(
                &mut sd,
                TRUE,
                Some(dacl.as_ptr() as *mut ACL),
                FALSE,
            ).expect(""Failed to set DACL"");
        }

        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: Box::into_raw(Box::new(sd)) as *mut _,
            bInheritHandle: FALSE,
        }
    }

    fn build_dacl(&self) -> Vec<u8> {
        // Calculate total size needed
        let header_size = std::mem::size_of::<ACL>();
        let ace_size: usize = self.ace_entries.iter()
            .map(|e| {
                // ACESIZE based on SID size + typical ACE header
                4 + 4 + 16 + e.sid.len() // AceHeader + Mask + SID
            })
            .sum();

        let mut dacl = vec![0u8; header_size + ace_size];

        // Initialize ACL
        unsafe {
            winapi::call!(
                ""windows::Win32::Security::Security::InitializeAcl"",
                fn(
                    pAcl: *mut u8,
                    nAclLength: u32,
                    dwAclRevision: u8
                ) -> BOOL
            )(
                dacl.as_mut_ptr(),
                dacl.len() as u32,
                ACL_REVISION,
            ).expect(""Failed to initialize ACL"");
        }

        // Add each ACE
        let mut offset = header_size;
        for ace in &self.ace_entries {
            let ace_header = (ACE_TYPE_ACCESS_ALLOWED << 16) | ACE_FLAG_INHERITED_ACE;
            let mask = FILE_ALL_ACCESS;

            // Write ACE header
            dacl[offset..offset+4].copy_from_slice(&[
                (ace_header & 0xFF) as u8,
                ((ace_header >> 8) & 0xFF) as u8,
                ((ace_header >> 16) & 0xFF) as u8,
                ((ace_header >> 24) & 0xFF) as u8,
            ]);
            offset += 4;

            // Write ACE size
            let ace_len = (4 + 4 + 16 + ace.sid.len()) as u16;
            dacl[offset..offset+2].copy_from_slice(&ace_len.to_le_bytes());
            offset += 2;

            // Write access mask
            dacl[offset..offset+4].copy_from_slice(&mask.to_le_bytes());
            offset += 4;

            // Write SID (simplified - actual implementation uses full SID structure)
            dacl[offset..offset+ace.sid.len()].copy_from_slice(&ace.sid);
            offset += ace.sid.len();
        }

        dacl
    }
}

impl Drop for SecurityAttributesBuilder {
    fn drop(&mut self) {
        // Security descriptor cleanup handled by caller
    }
}
`

## Named Pipe Server (Tokio Integration)

`ust
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncRead, AsyncWrite};
use tokio::net::windows::named_pipe::{NamedPipeServer, NamedPipeClient};
use std::pin::Pin;
use std::task::{Context, Poll};

/// Wrapper for Windows named pipe server with tokio async support
pub struct NamedPipeServer {
    server: Pin<Box<NamedPipeServer>>,
    overlapped: Overlapped,
}

impl NamedPipeServer {
    /// Create new named pipe server
    pub fn new(name: &str, security_attrs: &SECURITY_ATTRIBUTES) -> Result<Self> {
        let server = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .security_attrs(security_attrs)
            .open(format!(r""\\.\pipe\{}"", name))
            .map_err(|e| IpcError::PipeCreationFailed(e.to_string()))?;

        Ok(Self {
            server: Pin::new(Box::new(server)),
            overlapped: Overlapped::new(),
        })
    }

    /// Accept client connection (async)
    pub async fn accept(&self) -> Result<NamedPipeClient> {
        // Wait for client connection
        unsafe {
            let mut overlapped = Overlapped::new();
            let result = ConnectNamedPipe(
                self.server.as_raw_handle() as HANDLE,
                Some(overlapped.as_ptr()),
            );

            if result == 0 {
                let err = GetLastError();
                if err != ERROR_IO_PENDING {
                    return Err(IpcError::ConnectionFailed(format!(
                        ""ConnectNamedPipe failed: {}"",
                        err
                    )));
                }
            }

            // Wait for completion
            let bytes = overlapped.get_result()
                .map_err(|e| IpcError::ConnectionFailed(e.to_string()))?;

            if bytes == 0 {
                return Err(IpcError::ConnectionLost);
            }
        }

        // Convert to async client wrapper
        let client = NamedPipeClient::from_raw_handle(self.server.as_raw_handle());
        Ok(client)
    }
}

/// Wrapper for Windows named pipe client with tokio async support
pub struct NamedPipeClient {
    pipe: tokio::net::windows::named_pipe::NamedPipeClient,
}

impl NamedPipeClient {
    /// Connect to existing named pipe
    pub async fn connect(name: &str) -> Result<Self> {
        let pipe = tokio::net::windows::named_pipe::NamedPipeClient::connect(name)
            .map_err(|e| IpcError::ConnectionFailed(e.to_string()))?;

        Ok(Self { pipe })
    }

    /// Write data to pipe (async)
    pub async fn write_all(&mut self, data: &[u8]) -> Result<()> {
        // Write length prefix first
        let len = (data.len() as u32).to_le_bytes();
        self.pipe.write_all(&len).await?;

        // Write data
        self.pipe.write_all(data).await?;

        Ok(())
    }

    /// Read data from pipe (async)
    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.pipe.read_exact(buf).await?;
        Ok(())
    }
}
`

## Client Subscription System

`ust
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{RwLock, watch, unbounded};

#[derive(Debug, Clone)]
pub struct SubscriptionId(u64);

pub struct SubscriptionManager {
    subscriptions: Arc<RwLock<HashMap<u64, Subscription>>>,
    next_id: AtomicU64,
    event_tx: unbounded::Sender<IpcEvent>,
}

struct Subscription {
    /// Subscribed event types
    filter: HashSet<String>,

    /// Channel to send events to this client
    tx: unbounded::Sender<IpcEvent>,

    /// Close signal receiver
    close_rx: watch::Receiver<bool>,
}

impl SubscriptionManager {
    pub fn new() -> Self {
        let (event_tx, _) = unbounded();
        Self {
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            event_tx,
        }
    }

    /// Create new subscription
    pub async fn subscribe(
        &self,
        events: Vec<String>,
    ) -> (u64, unbounded::Receiver<IpcEvent>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = unbounded();

        let filter: HashSet<String> = events.into_iter().collect();

        let (close_tx, close_rx) = watch::channel(false);

        let sub = Subscription {
            filter,
            tx,
            close_rx,
        };

        self.subscriptions.write().await.insert(id, sub);

        (id, rx)
    }

    /// Remove subscription
    pub async fn unsubscribe(&self, id: u64) {
        self.subscriptions.write().await.remove(&id);
    }

    /// Broadcast event to matching subscriptions
    pub async fn broadcast(&self, event: &IpcEvent) {
        let event_name = format(""{:?}"", event);
        let mut to_remove = Vec::new();

        let subs = self.subscriptions.read().await;
        for (id, sub) in subs.iter() {
            if sub.filter.contains(&event_name) || sub.filter.contains(""*"") {
                if sub.tx.send(event.clone()).is_err() {
                    // Client disconnected
                    to_remove.push(*id);
                }
            }
        }

        drop(subs);

        // Cleanup disconnected clients
        for id in to_remove {
            self.subscriptions.write().await.remove(&id);
        }
    }
}
`

## Async I/O with OVERLAPPED

`ust
use std::io::{self, Result};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite};

/// OVERLAPPED structure for async pipe operations
#[repr(C)]
struct OVERLAPPED {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    h_event: HANDLE,
}

impl OVERLAPPED {
    fn new() -> Self {
        Self {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            h_event: HANDLE::default(),
        }
    }
}

/// Async read operation with OVERLAPPED
pub struct OverlappedRead {
    overlapped: OVERLAPPED,
    buffer: Vec<u8>,
}

impl OverlappedRead {
    pub fn new(buffer: Vec<u8>) -> Self {
        Self {
            overlapped: OVERLAPPED::new(),
            buffer,
        }
    }

    pub fn as_ptr(&mut self) -> *mut OVERLAPPED {
        &mut self.overlapped
    }

    pub fn get_result(self) -> io::Result<usize> {
        // Wait for operation to complete and return bytes transferred
        unsafe {
            let mut bytes = 0u32;
            let result = GetOverlappedResult(
                std::ptr::null_mut(), // hFile - would be pipe handle
                &mut self.overlapped,
                true,                 // bWait
            );

            if result.as_bool() {
                Ok(self.overlapped.internal_high as usize)
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}
`

## Pipe Operations API

### Server-Side (CreateNamedPipeW)

`ust
use windows::Win32::System::Pipe::{
    CreateNamedPipeW,
    ConnectNamedPipe,
    DisconnectNamedPipe,
    WaitNamedPipeW,
    PIPE_ACCESS_DUPLEX,
    PIPE_TYPE_MESSAGE,
    PIPE_READMODE_MESSAGE,
    PIPE_WAIT,
    PIPE_UNLIMITED_INSTANCES,
    NMPWAIT_WAIT_FOREVER,
};

/// Create named pipe with security attributes
pub fn create_pipe_server(
    name: &str,
    security_attrs: &SECURITY_ATTRIBUTES,
) -> Result<HANDLE> {
    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

    let pipe = unsafe {
        CreateNamedPipeW(
            PCWSTR::from_raw(wide_name.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,  // Access mode
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,  // Mode
            PIPE_UNLIMITED_INSTANCES,  // Max instances
            65536,  // Out buffer size
            65536,  // In buffer size
            0,      // Default timeout (ms)
            Some(security_attrs),  // Security attributes
        )
    };

    if pipe.is_invalid() {
        return Err(IpcError::PipeCreationFailed(
            std::io::Error::last_os_error().to_string()
        ));
    }

    Ok(pipe)
}
`

### Client-Side (CreateFileW + WaitNamedPipeW)

`ust
use windows::Win32::System::Pipe::{CreateFileW, WaitNamedPipeW};
use windows::Win32::System::IO::{ReadFile, WriteFile};
use windows::Win32::Foundation::{HANDLE, OVERLAPPED};

/// Wait for pipe and connect
pub async fn connect_to_pipe(name: &str) -> Result<HANDLE> {
    // Wait for pipe to be available
    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

    let available = unsafe {
        WaitNamedPipeW(
            PCWSTR::from_raw(wide_name.as_ptr()),
            NMPWAIT_WAIT_FOREVER,
        )
    };

    if !available {
        return Err(IpcError::PipeNotAvailable);
    }

    // Open the pipe
    let handle = unsafe {
        CreateFileW(
            PCWSTR::from_raw(wide_name.as_ptr()),
            GENERIC_READ | GENERIC_WRITE,
            0,  // No sharing
            None,  // Default security
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
    };

    if handle.is_invalid() {
        return Err(IpcError::ConnectionFailed(
            std::io::Error::last_os_error().to_string()
        ));
    }

    Ok(handle)
}
`

## Integration Points

### With Layout Module

- Layout state exposed via GetState
- TileRequest creates new tile positions
- WindowMove and WindowResize update layout

### With Window Module

- Window operations via WindowMove, WindowResize, WindowClose
- Window list via WindowList
- Window events via WindowOpened, WindowClosed, WindowFocused

### With Workspace Module

- Workspace operations via WorkspaceCreate, WorkspaceDestroy, WorkspaceFocus
- Workspace events via WorkspacesChanged, WorkspaceActivated

### With Config Module

- Config reload via ConfigReload
- Config events via ConfigLoaded

## CLI Integration

The wiri CLI uses IPC for external control:

`ash
# Get current state
wiri-ctl get-state

# Move window
wiri-ctl move-window --hwnd 123456 --x 100 --y 200

# Resize window
wiri-ctl resize-window --hwnd 123456 --width 800 --height 600

# List windows
wiri-ctl list-windows

# Subscribe to events
wiri-ctl events --subscribe WorkspacesChanged,WindowClosed
`

## Key Bindings

IPC can be triggered from key bindings in config:

`kdl
binds {
    Mod+Ctrl+R { spawn ""wiri-ctl reload-config""; }
    Mod+Shift+W { spawn ""wiri-ctl list-windows""; }
}
`

## Thread Safety

- IpcServer uses Arc<RwLock<...>> for shared state
- SubscriptionManager uses Arc<RwLock<...>> for subscriptions
- Non-blocking async operations via tokio
- Event broadcasting is serialized through subscription manager

## Buffer Sizes

- **Out buffer**: 65536 bytes
- **In buffer**: 65536 bytes
- **Message mode**: Enabled for discrete message boundaries
- **Overlap mode**: Enabled for async operations

## Implementation Notes

1. **Message Framing**: Each message is prefixed with a 4-byte length (little-endian u32)
2. **Keep-Alive**: Server periodically checks client connections
3. **Slow Client Handling**: Clients not reading events are disconnected after timeout
4. **Reconnection**: Clients may reconnect after disconnect
5. **UTF-8 Encoding**: All JSON uses UTF-8 encoding
