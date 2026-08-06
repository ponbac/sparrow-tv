use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MpvSnapshot {
    paused: Option<bool>,
    time_position: Option<f64>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    dropped_frames: Option<u64>,
    estimated_fps: Option<f64>,
}

#[cfg(desktop)]
mod desktop_mpv {
    use super::MpvSnapshot;
    use serde_json::{json, Value};
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        path::PathBuf,
        process::{Child, Command, Stdio},
        sync::Mutex,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    #[derive(Default)]
    pub struct MpvState(pub Mutex<Option<MpvProcess>>);

    pub struct MpvProcess {
        child: Child,
        socket: PathBuf,
        next_request_id: u64,
    }

    impl MpvProcess {
        fn spawn() -> Result<Self, String> {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_millis();
            let socket = std::env::temp_dir().join(format!(
                "sparrow-playback-probe-{}-{nonce}.sock",
                std::process::id()
            ));
            let child = Command::new("mpv")
                .arg("--no-config")
                .arg("--idle=yes")
                .arg("--force-window=yes")
                .arg("--terminal=no")
                .arg("--input-ipc-server")
                .arg(&socket)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("could not start mpv: {error}"))?;

            for _ in 0..50 {
                if socket.exists() {
                    return Ok(Self {
                        child,
                        socket,
                        next_request_id: 1,
                    });
                }
                thread::sleep(Duration::from_millis(50));
            }

            Err("mpv did not create its IPC socket within 2.5 seconds".into())
        }

        fn request(&mut self, command: Value) -> Result<Value, String> {
            let request_id = self.next_request_id;
            self.next_request_id += 1;
            let mut stream = UnixStream::connect(&self.socket)
                .map_err(|error| format!("could not connect to mpv IPC: {error}"))?;
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .map_err(|error| error.to_string())?;
            let request = json!({ "command": command, "request_id": request_id });
            writeln!(stream, "{request}").map_err(|error| error.to_string())?;

            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .map_err(|error| format!("could not read mpv IPC: {error}"))?;
                if line.is_empty() {
                    return Err("mpv closed its IPC connection".into());
                }
                let value: Value = serde_json::from_str(&line)
                    .map_err(|error| format!("invalid mpv IPC response: {error}"))?;
                if value.get("request_id").and_then(Value::as_u64) == Some(request_id) {
                    if value.get("error").and_then(Value::as_str) == Some("success") {
                        return Ok(value.get("data").cloned().unwrap_or(Value::Null));
                    }
                    return Err(format!(
                        "mpv command failed: {}",
                        value.get("error").and_then(Value::as_str).unwrap_or("unknown error")
                    ));
                }
            }
        }

        fn property(&mut self, name: &str) -> Option<Value> {
            self.request(json!(["get_property", name])).ok()
        }

        fn snapshot(&mut self) -> MpvSnapshot {
            MpvSnapshot {
                paused: self.property("pause").and_then(|value| value.as_bool()),
                time_position: self.property("time-pos").and_then(|value| value.as_f64()),
                video_codec: self
                    .property("video-codec")
                    .and_then(|value| value.as_str().map(ToOwned::to_owned)),
                audio_codec: self
                    .property("audio-codec-name")
                    .and_then(|value| value.as_str().map(ToOwned::to_owned)),
                dropped_frames: self
                    .property("decoder-frame-drop-count")
                    .and_then(|value| value.as_u64()),
                estimated_fps: self.property("estimated-vf-fps").and_then(|value| value.as_f64()),
            }
        }

        fn terminate(mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_file(&self.socket);
        }
    }

    pub fn start(state: &MpvState, url: String) -> Result<MpvSnapshot, String> {
        let mut guard = state.0.lock().map_err(|_| "mpv state lock poisoned")?;
        if let Some(process) = guard.take() {
            process.terminate();
        }
        let mut process = MpvProcess::spawn()?;
        process.request(json!(["loadfile", url, "replace"]))?;
        let snapshot = process.snapshot();
        *guard = Some(process);
        Ok(snapshot)
    }

    pub fn command(state: &MpvState, command: &str) -> Result<MpvSnapshot, String> {
        let mut guard = state.0.lock().map_err(|_| "mpv state lock poisoned")?;
        let process = guard.as_mut().ok_or("mpv is not running")?;
        match command {
            "pause" => {
                process.request(json!(["cycle", "pause"]))?;
            }
            "fullscreen" => {
                process.request(json!(["cycle", "fullscreen"]))?;
            }
            "reload" => {
                process.request(json!(["playlist-play-index", "current"]))?;
            }
            _ => return Err(format!("unsupported mpv command: {command}")),
        }
        Ok(process.snapshot())
    }

    pub fn snapshot(state: &MpvState) -> Result<MpvSnapshot, String> {
        let mut guard = state.0.lock().map_err(|_| "mpv state lock poisoned")?;
        Ok(guard.as_mut().ok_or("mpv is not running")?.snapshot())
    }

    pub fn stop(state: &MpvState) -> Result<(), String> {
        let mut guard = state.0.lock().map_err(|_| "mpv state lock poisoned")?;
        if let Some(process) = guard.take() {
            process.terminate();
        }
        Ok(())
    }
}

#[cfg(desktop)]
#[tauri::command]
fn mpv_start(
    state: tauri::State<'_, desktop_mpv::MpvState>,
    url: String,
) -> Result<MpvSnapshot, String> {
    desktop_mpv::start(&state, url)
}

#[cfg(mobile)]
#[tauri::command]
fn mpv_start(_url: String) -> Result<MpvSnapshot, String> {
    Err("mpv baseline is available only on Linux".into())
}

#[cfg(desktop)]
#[tauri::command]
fn mpv_command(
    state: tauri::State<'_, desktop_mpv::MpvState>,
    command: String,
) -> Result<MpvSnapshot, String> {
    desktop_mpv::command(&state, &command)
}

#[cfg(mobile)]
#[tauri::command]
fn mpv_command(_command: String) -> Result<MpvSnapshot, String> {
    Err("mpv baseline is available only on Linux".into())
}

#[cfg(desktop)]
#[tauri::command]
fn mpv_snapshot(
    state: tauri::State<'_, desktop_mpv::MpvState>,
) -> Result<MpvSnapshot, String> {
    desktop_mpv::snapshot(&state)
}

#[cfg(mobile)]
#[tauri::command]
fn mpv_snapshot() -> Result<MpvSnapshot, String> {
    Err("mpv baseline is available only on Linux".into())
}

#[cfg(desktop)]
#[tauri::command]
fn mpv_stop(state: tauri::State<'_, desktop_mpv::MpvState>) -> Result<(), String> {
    desktop_mpv::stop(&state)
}

#[cfg(mobile)]
#[tauri::command]
fn mpv_stop() -> Result<(), String> {
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_http::init());
    #[cfg(desktop)]
    let builder = builder.manage(desktop_mpv::MpvState::default());

    builder
        .invoke_handler(tauri::generate_handler![
            mpv_start,
            mpv_command,
            mpv_snapshot,
            mpv_stop
        ])
        .run(tauri::generate_context!())
        .expect("error while running Sparrow playback prototype");
}
