pub mod watcher;

pub use watcher::{AssetReloadEvent, AssetWatcher};
pub use wm_proto;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;
use wm_proto::{
    ActionCommand, IpcCommand, IpcPayload, IpcRequest, IpcResponse, OutputDescriptor, StateValue,
    WidgetStatus, WindowDescriptor, WorkspaceDescriptor, DEFAULT_SOCKET_NAME,
};

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("IPC I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("IPC JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IPC server error: {0}")]
    Server(String),
}

/// Returns the default Unix domain socket path for `lumina-wm`.
/// Priority: `$LUMINA_SOCKET` -> `$XDG_RUNTIME_DIR/lumina-wm.sock` -> `/tmp/lumina-wm.sock`.
pub fn default_socket_path() -> PathBuf {
    if let Some(custom) = std::env::var_os("LUMINA_SOCKET") {
        let p = PathBuf::from(custom);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(runtime_dir);
        if dir.exists() {
            return dir.join(DEFAULT_SOCKET_NAME);
        }
    }
    PathBuf::from("/tmp").join(DEFAULT_SOCKET_NAME)
}

/// Non-blocking and threaded Unix domain socket NDJSON IPC server.
pub struct IpcServer {
    socket_path: PathBuf,
    listener: UnixListener,
}

impl IpcServer {
    /// Binds a Unix domain socket listener at `socket_path` in non-blocking mode.
    pub fn bind(socket_path: impl Into<PathBuf>) -> Result<Self, IpcError> {
        let socket_path = socket_path.into();
        if let Some(parent) = socket_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        if socket_path.exists() {
            let _ = fs::remove_file(&socket_path);
        }

        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        Ok(Self {
            socket_path,
            listener,
        })
    }

    /// Binds using `default_socket_path()`.
    pub fn bind_default() -> Result<Self, IpcError> {
        Self::bind(default_socket_path())
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Non-blocking poll that accepts all pending client connections and processes their NDJSON requests.
    pub fn poll_once<F>(&mut self, mut handler: F) -> Result<usize, IpcError>
    where
        F: FnMut(IpcRequest) -> IpcResponse,
    {
        let mut processed = 0usize;
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    processed += Self::handle_client_stream(stream, &mut handler)?;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(IpcError::Io(err)),
            }
        }
        Ok(processed)
    }

    /// Spawns a background thread serving NDJSON `IpcRequest`s via `handler`.
    /// The socket is bound synchronously before `spawn` returns so clients can connect immediately.
    pub fn spawn<F>(
        socket_path: impl Into<PathBuf>,
        mut handler: F,
    ) -> Result<IpcServerHandle, IpcError>
    where
        F: FnMut(IpcRequest) -> IpcResponse + Send + 'static,
    {
        let server = Self::bind(socket_path)?;
        let path = server.socket_path.clone();
        let listener = server.listener.try_clone()?;
        // Prevent `server`'s Drop from unlinking the socket while the background thread runs
        std::mem::forget(server);

        let running = Arc::new(AtomicBool::new(true));
        let running_thread = Arc::clone(&running);
        let path_thread = path.clone();

        let join_handle = thread::spawn(move || {
            while running_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        if !running_thread.load(Ordering::SeqCst) {
                            break;
                        }
                        let _ = Self::handle_client_stream(stream, &mut handler);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            }
            let _ = fs::remove_file(&path_thread);
        });

        Ok(IpcServerHandle {
            socket_path: path,
            running,
            join_handle: Some(join_handle),
        })
    }

    fn handle_client_stream<F>(stream: UnixStream, handler: &mut F) -> Result<usize, IpcError>
    where
        F: FnMut(IpcRequest) -> IpcResponse,
    {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

        let mut writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream);
        let mut count = 0usize;
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = match reader.read_line(&mut line) {
                Ok(n) => n,
                Err(_) => break,
            };
            if bytes_read == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let response = match serde_json::from_str::<IpcRequest>(trimmed) {
                Ok(req) => handler(req),
                Err(err) => IpcResponse::err(0, format!("Invalid IPC JSON request: {err}")),
            };

            let mut encoded = serde_json::to_string(&response)?;
            encoded.push('\n');
            writer.write_all(encoded.as_bytes())?;
            writer.flush()?;
            count += 1;
        }

        Ok(count)
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

/// Handle to a background threaded `IpcServer`. Automatically shuts down on drop.
pub struct IpcServerHandle {
    socket_path: PathBuf,
    running: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl IpcServerHandle {
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn shutdown(&mut self) {
        if self.running.swap(false, Ordering::SeqCst) {
            let _ = UnixStream::connect(&self.socket_path);
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

impl Drop for IpcServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Unix domain socket NDJSON IPC client used by `luminactl` and tests.
pub struct IpcClient {
    socket_path: PathBuf,
    next_id: AtomicU64,
    timeout: Duration,
}

impl Default for IpcClient {
    fn default() -> Self {
        Self::from_env()
    }
}

impl IpcClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            next_id: AtomicU64::new(1),
            timeout: Duration::from_secs(5),
        }
    }

    pub fn from_env() -> Self {
        Self::new(default_socket_path())
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Sends an `IpcRequest` over the Unix domain socket and parses the `IpcResponse`.
    pub fn send_request(&self, request: &IpcRequest) -> Result<IpcResponse, IpcError> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        let _ = stream.set_read_timeout(Some(self.timeout));
        let _ = stream.set_write_timeout(Some(self.timeout));

        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        stream.flush()?;

        let mut reader = BufReader::new(stream);
        let mut resp_line = String::new();
        let n = reader.read_line(&mut resp_line)?;
        if n == 0 {
            return Err(IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "IPC server closed connection before responding",
            )));
        }

        let resp: IpcResponse = serde_json::from_str(resp_line.trim())?;
        Ok(resp)
    }

    /// Wraps an `IpcCommand` in an `IpcRequest` with an auto-incrementing ID and sends it.
    pub fn send(&self, command: IpcCommand) -> Result<IpcResponse, IpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = IpcRequest { id, command };
        self.send_request(&req)
    }

    pub fn ping(&self) -> Result<IpcResponse, IpcError> {
        self.send(IpcCommand::Ping)
    }

    pub fn dispatch_dsl(&self, dsl: &str) -> Result<Vec<IpcResponse>, IpcError> {
        let cmds = ActionCommand::parse_dsl_list(dsl).map_err(IpcError::Server)?;
        let mut responses = Vec::with_capacity(cmds.len());
        for action in cmds {
            responses.push(self.send(IpcCommand::WmDispatch { action })?);
        }
        Ok(responses)
    }

    pub fn wm_workspaces(&self) -> Result<Vec<WorkspaceDescriptor>, IpcError> {
        let resp = self.send(IpcCommand::WmWorkspaces)?;
        if !resp.ok {
            return Err(IpcError::Server(
                resp.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }
        match resp.payload {
            Some(IpcPayload::Workspaces(ws)) => Ok(ws),
            other => Err(IpcError::Server(format!(
                "Unexpected payload for WmWorkspaces: {other:?}"
            ))),
        }
    }

    pub fn wm_windows(&self) -> Result<Vec<WindowDescriptor>, IpcError> {
        let resp = self.send(IpcCommand::WmWindows)?;
        if !resp.ok {
            return Err(IpcError::Server(
                resp.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }
        match resp.payload {
            Some(IpcPayload::Windows(wins)) => Ok(wins),
            other => Err(IpcError::Server(format!(
                "Unexpected payload for WmWindows: {other:?}"
            ))),
        }
    }

    pub fn wm_outputs(&self) -> Result<Vec<OutputDescriptor>, IpcError> {
        let resp = self.send(IpcCommand::WmOutputs)?;
        if !resp.ok {
            return Err(IpcError::Server(
                resp.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }
        match resp.payload {
            Some(IpcPayload::Outputs(outs)) => Ok(outs),
            other => Err(IpcError::Server(format!(
                "Unexpected payload for WmOutputs: {other:?}"
            ))),
        }
    }

    pub fn widget_list(&self) -> Result<Vec<WidgetStatus>, IpcError> {
        let resp = self.send(IpcCommand::WidgetList)?;
        if !resp.ok {
            return Err(IpcError::Server(
                resp.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }
        match resp.payload {
            Some(IpcPayload::Widgets(widgets)) => Ok(widgets),
            other => Err(IpcError::Server(format!(
                "Unexpected payload for WidgetList: {other:?}"
            ))),
        }
    }

    pub fn state_get(&self, path: Option<String>) -> Result<StateValue, IpcError> {
        let resp = self.send(IpcCommand::StateGet { path })?;
        if !resp.ok {
            return Err(IpcError::Server(
                resp.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }
        match resp.payload {
            Some(IpcPayload::State(val)) => Ok(val),
            other => Err(IpcError::Server(format!(
                "Unexpected payload for StateGet: {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wm_proto::{LayoutMode, ReloadTarget};

    #[test]
    fn test_ipc_server_and_client_unix_socket_roundtrip() {
        let sock_path = std::env::temp_dir().join(format!(
            "lumina-ipc-test-{}.sock",
            std::process::id()
        ));

        let mut server_handle = IpcServer::spawn(&sock_path, |req| match req.command {
            IpcCommand::Ping => IpcResponse::success(
                req.id,
                IpcPayload::Pong {
                    version: "0.1.0".to_string(),
                },
            ),
            IpcCommand::WmWorkspaces => IpcResponse::success(
                req.id,
                IpcPayload::Workspaces(vec![WorkspaceDescriptor {
                    id: 1,
                    idx: 1,
                    name: "1".to_string(),
                    output: "eDP-1".to_string(),
                    layout_mode: LayoutMode::Tiling,
                    is_active: true,
                    is_focused: true,
                    is_urgent: false,
                    window_count: 2,
                }]),
            ),
            IpcCommand::StateGet { path } => IpcResponse::success(
                req.id,
                IpcPayload::State(StateValue::String(
                    path.unwrap_or_else(|| "root".to_string()),
                )),
            ),
            _ => IpcResponse::err(req.id, "Unsupported in mock"),
        })
        .expect("should bind and spawn IPC server");

        let client = IpcClient::new(&sock_path);

        // 1. Ping roundtrip
        let ping_resp = client.ping().expect("ping should succeed");
        assert!(ping_resp.ok);
        assert_eq!(
            ping_resp.payload,
            Some(IpcPayload::Pong {
                version: "0.1.0".to_string()
            })
        );

        // 2. Workspaces query roundtrip
        let workspaces = client.wm_workspaces().expect("workspaces should succeed");
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, 1);
        assert_eq!(workspaces[0].window_count, 2);

        // 3. StateGet roundtrip
        let state_val = client
            .state_get(Some("wm.active_workspace".to_string()))
            .expect("state_get should succeed");
        assert_eq!(
            state_val,
            StateValue::String("wm.active_workspace".to_string())
        );

        server_handle.shutdown();
        assert!(!sock_path.exists());
    }

    #[test]
    fn test_asset_watcher_detects_css_and_html_changes() {
        let dir = std::env::temp_dir().join(format!("lumina-watcher-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let css_path = dir.join("theme.css");
        let html_path = dir.join("panel.html");
        fs::write(&css_path, ":root { --accent: #38bdf8; }").unwrap();

        let mut watcher = AssetWatcher::new(&dir);
        assert!(watcher.poll_events().is_empty());

        // Sleep briefly so mtime advances, then create panel.html and modify theme.css
        thread::sleep(Duration::from_millis(20));
        fs::write(&html_path, "<div class='panel'></div>").unwrap();
        fs::write(&css_path, ":root { --accent: #f43f5e; }").unwrap();

        let targets = watcher.poll_reload_targets();
        assert!(targets.contains(&ReloadTarget::Html));
        assert!(targets.contains(&ReloadTarget::Css));

        let _ = fs::remove_dir_all(&dir);
    }
}
