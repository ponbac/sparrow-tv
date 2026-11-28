use std::{cell::Cell, env, path::PathBuf, rc::Rc, sync::Arc};

use gpui::{AppContext, Bounds, QuitMode, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use sparrow_guide_ui::{Catalog, GuideHost, GuideSurface, register_fonts};
use tokio::sync::mpsc;

mod agent_control;
mod host;
mod playback;

use host::DesktopHost;
use playback::DesktopPlayback;

const USAGE: &str = "Usage: sparrow-desktop [--data-dir DIRECTORY | --fixture]\n\
    Click, scroll or use the arrow keys. Enter selects. Escape goes back.\n\
    / or Ctrl+F searches, F toggles full screen, Space pauses, M mutes, G returns to the Guide.\n\
    --fixture uses a temporary data directory and the loopback Shield fixture on port 18108.";

pub(super) fn run() -> Result<(), &'static str> {
    let mut args = env::args_os().skip(1);
    let option = args.next();
    let mut fixture_dir = None;
    let root = match option.as_deref().and_then(|arg| arg.to_str()) {
        None if option.is_none() => default_data_dir()?,
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some("--data-dir") => args.next().map(PathBuf::from).ok_or(USAGE)?,
        Some("--fixture") => {
            let directory = tempfile::Builder::new()
                .prefix("sparrow-desktop-fixture-")
                .tempdir()
                .map_err(|_| "Couldn’t create a temporary fixture directory.")?;
            let root = directory.path().to_owned();
            fixture_dir = Some(directory);
            root
        }
        _ => return Err(USAGE),
    };
    if args.next().is_some() {
        return Err(USAGE);
    }

    let playback = Arc::new(DesktopPlayback::default());
    let catalog = Arc::new(Catalog::new(Some(playback.clone())));
    catalog.start(root);
    let (agent, mut intents) = mpsc::channel(8);
    let agent = agent_control::Server::start(catalog.clone(), playback.clone(), agent);
    let opened = Rc::new(Cell::new(false));
    let status = Rc::clone(&opened);
    let guide_catalog = Arc::clone(&catalog);
    let use_fixture = fixture_dir.is_some();
    gpui_platform::application().run(move |cx| {
        let catalog = guide_catalog;
        cx.set_quit_mode(QuitMode::LastWindowClosed);
        register_fonts(cx);

        let (events, mut pending) = mpsc::unbounded_channel();
        let host = Rc::new(DesktopHost::new(events));
        let id = host.id();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Sparrow TV".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.), px(720.)),
                cx,
            ))),
            window_min_size: Some(size(px(480.), px(360.))),
            app_id: Some("xyz.ponbac.sparrow".into()),
            ..Default::default()
        };

        let Ok(window) = cx.open_window(options, |window, cx| {
            cx.new(|cx| GuideSurface::new(host, Arc::clone(&catalog), window, cx))
        }) else {
            cx.quit();
            return;
        };
        status.set(true);
        playback.present(window, cx);
        let fixture = use_fixture.then(|| {
            catalog.seed_fixture(
                "http://127.0.0.1:18108/catalog.m3u".into(),
                "http://127.0.0.1:18108/guide.xml".into(),
            )
        });
        let catalog = Arc::clone(&catalog);
        cx.spawn(async move |cx| {
            if let Some(result) = fixture {
                let ready = match result {
                    Some(result) => result.await.unwrap_or(false),
                    None => false,
                };
                if !ready {
                    eprintln!("Couldn’t load the loopback Channel Catalog fixture.");
                }
            }
            catalog.set_active(id, true);
            loop {
                tokio::select! {
                    event = pending.recv() => {
                        let Some(event) = event else { break };
                        if cx.update(|cx| window.update(cx, |view, window, cx| {
                            view.host_event(event, window, cx)
                        })).is_err() {
                            break;
                        }
                    }
                    Some(intent) = intents.recv() => {
                        if cx.update(|cx| window.update(cx, |view, _, _| {
                            intent.commit(|command| view.external_playback_intent(command));
                        })).is_err() {
                            break;
                        }
                    }
                }
            }
            catalog.host_destroyed(id, true);
        })
        .detach();
        cx.activate(true);
    });
    if let Some(runtime) = catalog.runtime() {
        runtime.block_on(async {
            if let Some(agent) = agent {
                agent.shutdown().await;
            }
            if let Some(runner) = catalog.runner()
                && runner.stop().await.is_err()
            {
                eprintln!("Couldn’t confirm Playback Session cleanup.");
            }
        });
    }
    drop(catalog);
    opened
        .get()
        .then_some(())
        .ok_or("Couldn’t open the Sparrow TV window.")
}

fn default_data_dir() -> Result<PathBuf, &'static str> {
    if let Some(root) = env::var_os("SPARROW_DATA_DIR").filter(|root| !root.is_empty()) {
        return Ok(root.into());
    }
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or("Set SPARROW_DATA_DIR or pass --data-dir.")?;
    Ok(data_home.join("xyz.ponbac.sparrow"))
}
