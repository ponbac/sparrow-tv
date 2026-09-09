//! Opt-in, bounded measurements of the real installed UI and native byte bridge.
//! Compiled out of normal builds. Never records source or catalog values.
use serde::{Deserialize, Serialize};
use tauri::{Manager, webview::PageLoadEvent};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Distribution {
    count: u32,
    p50: f64,
    p95: f64,
    max: f64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SeekWrite {
    from: f64,
    to: f64,
    buffer_end: f64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Sample {
    elapsed: u32,
    frame_gaps: Distribution,
    media_gaps: Distribution,
    raf_gaps: Distribution,
    seek_writes: Vec<SeekWrite>,
    waiting: u32,
    seeking: u32,
    seeked: u32,
    buffer_ahead: f64,
    selected: bool,
    fullscreen: bool,
    mpv: bool,
    playing: bool,
    failed: bool,
    time: f64,
    total: u64,
    dropped: u64,
    presented: u64,
    ready: u8,
    paused: bool,
    width: u32,
    height: u32,
}

pub(crate) fn on_page_load(
    webview: &tauri::Webview,
    payload: &tauri::webview::PageLoadPayload<'_>,
) {
    if payload.event() != PageLoadEvent::Finished {
        return;
    }
    let Some(duration) = std::env::var("SPARROW_PLAYBACK_LAB_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (10..=900).contains(value))
    else {
        return;
    };
    let switch_players = std::env::var("SPARROW_PLAYBACK_LAB_SWITCH_PLAYERS").as_deref() == Ok("1");
    let webview = webview.clone();
    tauri::async_runtime::spawn(async move {
        for elapsed in 0..=duration {
            let script =
                include_str!("../../../scripts/debug/linux-playback-lab/installed-probe.js")
                    .replace("__ELAPSED__", &elapsed.to_string())
                    .replace(
                        "__SWITCH_PLAYERS__",
                        if switch_players { "true" } else { "false" },
                    );
            if webview
                .eval_with_callback(script, |result| {
                    if let Ok(sample) = serde_json::from_str::<Sample>(&result)
                        && let Ok(json) = serde_json::to_string(&sample)
                    {
                        println!("PLAYBACK_LAB {json}");
                    }
                })
                .is_err()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        let _ = webview.eval("[...document.querySelectorAll('button')].find(b => ['Stop stream', 'Stop mpv', 'Close player'].includes(b.textContent.trim()))?.click()");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        println!("PLAYBACK_LAB finished");
        webview.app_handle().exit(0);
    });
}
