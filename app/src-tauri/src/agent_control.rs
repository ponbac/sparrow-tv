//! Opt-in local Agent Control: a Unix-socket command surface for the installed UI.
//! Binds only when SPARROW_AGENT_SOCKET and SPARROW_AGENT_TOKEN are set. Never
//! logs the token or search terms.

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use crate::ipc::dto::ClientErrorDto;

const MAX_SOCKET_PATH_BYTES: usize = 100;
const MIN_TOKEN_BYTES: usize = 16;
const MAX_TOKEN_BYTES: usize = 128;
const MAX_REQUEST_BYTES: usize = 32 * 1024;
const MAX_SEARCH_TERM_BYTES: usize = 256;
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(15);
const EVAL_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONNECTIONS: usize = 8;

/// Completes one in-flight Agent Control eval from the installed webview.
#[derive(Clone, Default)]
pub(crate) struct ReplyMailbox {
    inner: Arc<MailboxInner>,
}

struct MailboxInner {
    next: AtomicU64,
    pending: Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>,
}

impl Default for MailboxInner {
    fn default() -> Self {
        Self {
            next: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
        }
    }
}

/// Completes a pending Agent Control request. Unknown ids are ignored.
#[tauri::command]
pub(crate) fn agent_control_reply(
    id: String,
    body: serde_json::Value,
    mailbox: State<'_, ReplyMailbox>,
) -> Result<(), ClientErrorDto> {
    mailbox.complete(&id, body);
    Ok(())
}

/// Starts the Agent Control listener when the process environment opts in.
pub(crate) fn start(app: AppHandle) {
    #[cfg(target_os = "linux")]
    start_linux(app);
    #[cfg(not(target_os = "linux"))]
    let _ = app;
}

#[cfg(target_os = "linux")]
fn start_linux(app: AppHandle) {
    let Some(config) = parse_config(
        std::env::var("SPARROW_AGENT_SOCKET").ok().as_deref(),
        std::env::var("SPARROW_AGENT_TOKEN").ok().as_deref(),
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .as_deref(),
    ) else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        if let Err(_error) = serve(app, config).await {
            eprintln!("sparrow-agent-control: unavailable");
        }
    });
}

fn parse_config(
    socket: Option<&str>,
    token: Option<&str>,
    runtime_dir: Option<&Path>,
) -> Option<AgentControlConfig> {
    let socket = socket?;
    let token = token?;
    if !token_is_allowed(token) || !socket_path_is_allowed(Path::new(socket), runtime_dir) {
        eprintln!("sparrow-agent-control: unavailable");
        return None;
    }
    Some(AgentControlConfig {
        socket: PathBuf::from(socket),
        token: token.to_owned(),
    })
}

struct AgentControlConfig {
    socket: PathBuf,
    token: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "_tag", rename_all = "kebab-case", deny_unknown_fields)]
enum AgentRequest {
    Ping,
    Search { term: String },
    Tune { term: String },
    Snapshot,
    Stop,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    token: String,
    request: AgentRequest,
}

impl ReplyMailbox {
    fn allocate(&self, sender: oneshot::Sender<serde_json::Value>) -> String {
        let sequence = self.inner.next.fetch_add(1, Ordering::Relaxed);
        let id = format!("agnt1_{sequence:016x}");
        self.inner
            .pending
            .lock()
            .expect("agent control mailbox poisoned")
            .insert(id.clone(), sender);
        id
    }

    fn complete(&self, id: &str, body: serde_json::Value) {
        if let Some(sender) = self
            .inner
            .pending
            .lock()
            .expect("agent control mailbox poisoned")
            .remove(id)
        {
            let _ = sender.send(body);
        }
    }

    fn cancel(&self, id: &str) {
        self.inner
            .pending
            .lock()
            .expect("agent control mailbox poisoned")
            .remove(id);
    }
}

#[cfg(target_os = "linux")]
async fn serve(app: AppHandle, config: AgentControlConfig) -> Result<(), ()> {
    use std::os::unix::fs::PermissionsExt as _;

    prepare_socket_path(&config.socket)?;
    let listener = tokio::net::UnixListener::bind(&config.socket).map_err(|_| ())?;
    std::fs::set_permissions(&config.socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| ())?;
    eprintln!("sparrow-agent-control: listening");
    let connections = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        // Reject excess clients before allocating a task or waiting for input.
        let Ok(permit) = connections.clone().try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let app = app.clone();
        let token = config.token.clone();
        tauri::async_runtime::spawn(async move {
            let _permit = permit;
            let _ = handle_connection(app, token, stream).await;
        });
    }
}

#[cfg(target_os = "linux")]
fn prepare_socket_path(path: &Path) -> Result<(), ()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
        Ok(metadata) => {
            use std::os::unix::fs::FileTypeExt as _;
            if metadata.file_type().is_symlink() {
                return Err(());
            }
            if metadata.file_type().is_socket() {
                std::fs::remove_file(path).map_err(|_| ())
            } else {
                Err(())
            }
        }
    }
}

#[cfg(target_os = "linux")]
async fn handle_connection(
    app: AppHandle,
    token: String,
    stream: tokio::net::UnixStream,
) -> Result<(), ()> {
    let mut stream = BufReader::new(stream);
    let buf = match read_request(&mut stream, READ_TIMEOUT).await {
        Ok(buf) => buf,
        Err(()) => return write_error(&mut stream, "invalid-request").await,
    };
    let envelope = match serde_json::from_slice::<Envelope>(&buf) {
        Ok(envelope) => envelope,
        Err(_) => return write_error(&mut stream, "invalid-request").await,
    };
    if !tokens_match(&envelope.token, &token) {
        return write_error(&mut stream, "unauthenticated").await;
    }
    if !request_terms_are_bounded(&envelope.request) {
        return write_error(&mut stream, "invalid-request").await;
    }
    let mailbox = app.state::<ReplyMailbox>().inner().clone();
    let Some(window) = app.get_webview_window("main") else {
        return write_error(&mut stream, "unavailable").await;
    };
    let (sender, receiver) = oneshot::channel();
    let id = mailbox.allocate(sender);
    // Drop cleans up on eval errors, deadlines, disconnects and task cancellation.
    let _pending = PendingReply {
        mailbox: mailbox.clone(),
        id: id.clone(),
    };
    let _cancel = CancelDispatch {
        window: window.clone(),
        id: id.clone(),
    };
    let script = dispatch_script(&id, &envelope.request).map_err(|_| ())?;
    let (eval_tx, eval_rx) = oneshot::channel();
    let eval_tx = std::sync::Mutex::new(Some(eval_tx));
    window
        .eval_with_callback(script, move |result| {
            if let Some(sender) = eval_tx.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = sender.send(result);
            }
        })
        .map_err(|_| ())?;
    let eval = tokio::time::timeout(EVAL_TIMEOUT, eval_rx)
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    if !eval_is_queued(&eval) {
        mailbox.cancel(&id);
        return write_error(&mut stream, "unavailable").await;
    }
    let mut extra = [0_u8; 1];
    tokio::select! {
        // One request per connection. EOF or extra input cancels dispatch.
        _ = stream.read(&mut extra) => Err(()),
        reply = tokio::time::timeout(DISPATCH_TIMEOUT, receiver) => {
            match reply {
                Ok(Ok(body)) => write_json(&mut stream, &body).await,
                Ok(Err(_)) => write_error(&mut stream, "unavailable").await,
                Err(_) => write_error(&mut stream, "timeout").await,
            }
        }
    }
}

struct PendingReply {
    mailbox: ReplyMailbox,
    id: String,
}

impl Drop for PendingReply {
    fn drop(&mut self) {
        self.mailbox.cancel(&self.id);
    }
}

#[cfg(target_os = "linux")]
struct CancelDispatch {
    window: tauri::WebviewWindow,
    id: String,
}

#[cfg(target_os = "linux")]
impl Drop for CancelDispatch {
    fn drop(&mut self) {
        if let Ok(id) = serde_json::to_string(&self.id) {
            let _ = self
                .window
                .eval(format!("globalThis.__sparrowAgentControlCancel?.({id})"));
        }
    }
}

// Fixed storage is allocated before reading: an unterminated line cannot grow
// a Vec. One absolute deadline also bounds clients that drip bytes forever.
#[cfg(target_os = "linux")]
async fn read_request(
    stream: &mut BufReader<tokio::net::UnixStream>,
    deadline: Duration,
) -> Result<Vec<u8>, ()> {
    tokio::time::timeout(deadline, async {
        let mut storage = [0_u8; MAX_REQUEST_BYTES];
        for index in 0..storage.len() {
            storage[index] = stream.read_u8().await.map_err(|_| ())?;
            if storage[index] == b'\n' {
                return Ok(storage[..index].to_vec());
            }
        }
        Err(())
    })
    .await
    .map_err(|_| ())?
}

fn request_terms_are_bounded(request: &AgentRequest) -> bool {
    match request {
        AgentRequest::Search { term } | AgentRequest::Tune { term } => {
            let bytes = term.len();
            bytes > 0 && bytes <= MAX_SEARCH_TERM_BYTES
        }
        AgentRequest::Ping | AgentRequest::Snapshot | AgentRequest::Stop => true,
    }
}

fn dispatch_script(id: &str, request: &AgentRequest) -> Result<String, ()> {
    let id_json = serde_json::to_string(id).map_err(|_| ())?;
    let request_json = serde_json::to_string(request).map_err(|_| ())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ())?
        .as_millis();
    let starts_by = now + EVAL_TIMEOUT.as_millis();
    let expires_at = now + 12_000;
    Ok(format!(
        "(() => {{ try {{ if (Date.now() >= {starts_by}) {{ return \"expired\"; }} const dispatch = globalThis.__sparrowAgentControl; if (typeof dispatch !== \"function\") {{ return \"missing\"; }} dispatch({id_json}, {request_json}, {expires_at}); return \"queued\"; }} catch (_error) {{ return \"throw\"; }} }})()"
    ))
}

fn eval_is_queued(result: &str) -> bool {
    result == "queued"
        || result == "\"queued\""
        || serde_json::from_str::<String>(result).ok().as_deref() == Some("queued")
}

#[cfg(target_os = "linux")]
async fn write_error(stream: &mut BufReader<tokio::net::UnixStream>, tag: &str) -> Result<(), ()> {
    write_json(stream, &json!({ "ok": false, "error": { "_tag": tag } })).await
}

#[cfg(target_os = "linux")]
async fn write_json(
    stream: &mut BufReader<tokio::net::UnixStream>,
    value: &serde_json::Value,
) -> Result<(), ()> {
    let mut payload = serde_json::to_vec(value).map_err(|_| ())?;
    payload.push(b'\n');
    tokio::time::timeout(WRITE_TIMEOUT, async {
        stream.write_all(&payload).await.map_err(|_| ())?;
        stream.flush().await.map_err(|_| ())
    })
    .await
    .map_err(|_| ())?
}

fn socket_path_is_allowed(path: &Path, runtime_dir: Option<&Path>) -> bool {
    if !path.is_absolute() || path.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
        return false;
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return false;
    }
    if path.starts_with("/tmp") {
        return true;
    }
    runtime_dir.is_some_and(|runtime| {
        !runtime.as_os_str().is_empty() && runtime.is_absolute() && path.starts_with(runtime)
    })
}

fn token_is_allowed(token: &str) -> bool {
    let len = token.len();
    (MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&len)
        && token.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

fn tokens_match(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mailbox_scope_cleans_up_on_task_cancellation_and_late_reply() {
        let mailbox = ReplyMailbox::default();
        let (sender, receiver) = oneshot::channel();
        let id = mailbox.allocate(sender);
        let pending = PendingReply {
            mailbox: mailbox.clone(),
            id: id.clone(),
        };
        let task = tokio::spawn(async move {
            let _pending = pending;
            std::future::pending::<()>().await;
        });
        task.abort();
        let _ = task.await;
        assert!(receiver.await.is_err());
        mailbox.complete(&id, json!({"ok": true}));
        assert!(mailbox.inner.pending.lock().unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn socket_read_rejects_oversize_and_unframed_eof() {
        for payload in [vec![b'x'; MAX_REQUEST_BYTES], b"{}".to_vec()] {
            let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
            let writer = tokio::spawn(async move {
                client.write_all(&payload).await.unwrap();
                client.shutdown().await.unwrap();
            });
            assert!(
                read_request(&mut BufReader::new(server), READ_TIMEOUT)
                    .await
                    .is_err()
            );
            writer.await.unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn socket_read_deadline_is_absolute_even_with_partial_input() {
        let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
        client.write_all(b"{").await.unwrap();
        assert!(
            read_request(&mut BufReader::new(server), Duration::from_millis(10))
                .await
                .is_err()
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn socket_read_accepts_a_framed_request() {
        let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
        client.write_all(b"{}\n").await.unwrap();
        assert_eq!(
            read_request(&mut BufReader::new(server), READ_TIMEOUT)
                .await
                .unwrap(),
            b"{}"
        );
    }

    #[test]
    fn ignores_agent_control_without_both_variables() {
        assert!(parse_config(None, Some("0123456789abcdef"), None).is_none());
        assert!(parse_config(Some("/tmp/sparrow-agent.sock"), None, None).is_none());
    }

    #[test]
    fn accepts_a_tmp_socket_and_printable_token() {
        let config = parse_config(
            Some("/tmp/sparrow-agent.sock"),
            Some("0123456789abcdef"),
            None,
        )
        .expect("valid local Agent Control configuration");
        assert_eq!(config.socket, PathBuf::from("/tmp/sparrow-agent.sock"));
    }

    #[test]
    fn rejects_relative_sockets_parent_paths_and_short_tokens() {
        assert!(parse_config(Some("sparrow-agent.sock"), Some("0123456789abcdef"), None).is_none());
        assert!(
            parse_config(
                Some("/tmp/../etc/sparrow-agent.sock"),
                Some("0123456789abcdef"),
                None,
            )
            .is_none()
        );
        assert!(parse_config(Some("/tmp/sparrow-agent.sock"), Some("short"), None).is_none());
    }

    #[test]
    fn accepts_sockets_under_the_runtime_directory() {
        let runtime = Path::new("/run/user/1000");
        assert!(
            parse_config(
                Some("/run/user/1000/sparrow-agent.sock"),
                Some("0123456789abcdef"),
                Some(runtime),
            )
            .is_some()
        );
        assert!(
            parse_config(
                Some("/home/ponbac/sparrow-agent.sock"),
                Some("0123456789abcdef"),
                Some(runtime),
            )
            .is_none()
        );
    }

    #[test]
    fn parses_a_tune_envelope_and_rejects_unknown_fields() {
        let envelope: Envelope = serde_json::from_str(
            r#"{"token":"0123456789abcdef","request":{"_tag":"tune","term":"Eurosport 1 FHD SE"}}"#,
        )
        .expect("valid envelope");
        assert!(tokens_match(&envelope.token, "0123456789abcdef"));
        assert!(request_terms_are_bounded(&envelope.request));
        assert!(
            serde_json::from_str::<Envelope>(
                r#"{"token":"0123456789abcdef","request":{"_tag":"tune","term":"x"},"extra":1}"#,
            )
            .is_err()
        );
    }
}
