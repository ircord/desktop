#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod server;

use sha2::{Digest, Sha256};
use std::{error::Error, sync::Mutex, time::Duration};
use tauri::{
    AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    path::BaseDirectory,
    webview::{
        NewWindowResponse,
        cookie::{Cookie, SameSite},
    },
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_shell::{
    ShellExt,
    process::{CommandChild, CommandEvent},
};

const LOOPBACK: &str = "http://127.0.0.1:47832";

#[derive(Default)]
struct Frontend {
    child: Mutex<Option<CommandChild>>,
    connecting: tauri::async_runtime::Mutex<()>,
}

fn setup_window(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("index.html".into()))
        .title("IRCord")
        .inner_size(820., 600.)
        .min_inner_size(340., 350.)
        .center()
        .build()?;
    Ok(())
}

fn open_window(
    app: &AppHandle,
    address: Url,
    key: Option<String>,
    profile: Option<&str>,
) -> tauri::Result<()> {
    let initial = if key.is_some() {
        Url::parse("about:blank").unwrap()
    } else {
        address.clone()
    };
    let open = app.clone();
    let local_origin = address.origin();
    let navigation_origin = local_origin.clone();
    let mut window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(initial))
        .title("IRCord")
        .inner_size(1280., 840.)
        .disable_drag_drop_handler()
        // WebView cookies are scoped to the loopback host, across ports. Keep the
        // hosted instance's session and cache out of custom backends and dev.
        .incognito(profile.is_none())
        .visible(key.is_none())
        // HTTPS frames support the frontend's media embeds. Keep other loopback
        // origins out of this window and give remote pages no native permissions.
        .on_navigation(move |url| {
            url.origin() == navigation_origin
                || url.as_str() == "about:blank"
                || (url.scheme() == "https" && url.host_str() != Some("127.0.0.1"))
        })
        .on_new_window(move |url, _| {
            if url.origin() == local_origin {
                if let Some(window) = open.get_webview_window("main") {
                    let _ = window.navigate(url);
                }
            } else if matches!(url.scheme(), "http" | "https") {
                let _ = open.opener().open_url(url.as_str(), None::<&str>);
            }
            NewWindowResponse::Deny
        });
    if let Some(profile) = profile {
        let fingerprint = Sha256::digest(profile.as_bytes());
        #[cfg(target_os = "macos")]
        {
            let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
            window = if version.majorVersion >= 14 {
                window.data_store_identifier(fingerprint[..16].try_into().unwrap())
            } else {
                window.incognito(true)
            };
        }
        #[cfg(not(target_os = "macos"))]
        {
            let directory = app
                .path()
                .app_local_data_dir()?
                .join(format!("server-{fingerprint:x}"));
            window = window.data_directory(directory);
        }
    }
    let window = window.build()?;
    if let Some(key) = key {
        let cookie = Cookie::build(("ircord_desktop", key))
            .domain("127.0.0.1")
            .path("/")
            .http_only(true)
            .same_site(SameSite::Strict)
            .build();
        window.set_cookie(cookie)?;
        window.navigate(address)?;
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

fn show_error(app: &AppHandle) {
    let exit = app.clone();
    app.dialog()
        .message("IRCord could not open its local frontend. Close any other IRCord windows and reopen the app.")
        .title("IRCord")
        .kind(MessageDialogKind::Error)
        .show(move |_| exit.exit(1));
}

fn start_frontend(app: &AppHandle, backend: &Url) -> Result<(), Box<dyn Error>> {
    let frontend = app.state::<Frontend>();
    let mut running = frontend.child.lock().unwrap();
    if running.is_some() {
        return Ok(());
    }
    let address = Url::parse(LOOPBACK)?;
    let backend_origin = backend.origin().ascii_serialization();
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    let key = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let entry = app
        .path()
        .resolve("frontend/server/index.mjs", BaseDirectory::Resource)?;
    let (mut events, child) = app
        .shell()
        .sidecar("ircord-bun")?
        .args(["--no-env-file".as_ref(), entry.as_os_str()])
        .env_clear()
        .env("NITRO_HOST", "127.0.0.1")
        .env("NITRO_PORT", address.port().unwrap().to_string())
        .env("NUXT_BACKEND_URL", &backend_origin)
        .env("NUXT_SITE_URL", LOOPBACK)
        .env("NUXT_PUBLIC_INSTANCE_URL", &backend_origin)
        .env("IRCORD_DESKTOP_KEY", &key)
        .spawn()?;

    *running = Some(child);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Trust readiness from our own child. Another process may hold the port.
        let ready_line = format!("Listening on {address}...");
        let ready = tokio::time::timeout(Duration::from_secs(20), async {
            while let Some(event) = events.recv().await {
                match event {
                    CommandEvent::Stdout(line)
                        if String::from_utf8_lossy(&line).trim_end() == ready_line =>
                    {
                        return true;
                    }
                    CommandEvent::Terminated(_) | CommandEvent::Error(_) => break,
                    _ => {}
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        if !ready {
            show_error(&app);
            return;
        }
        let handle = app.clone();
        if app
            .run_on_main_thread(move || {
                if open_window(&handle, address, Some(key), Some(&backend_origin)).is_err() {
                    show_error(&handle);
                } else if let Some(setup) = handle.get_webview_window("setup") {
                    let _ = setup.close();
                }
            })
            .is_err()
        {
            app.exit(1);
        }
        while let Some(event) = events.recv().await {
            if matches!(event, CommandEvent::Terminated(_) | CommandEvent::Error(_)) {
                show_error(&app);
                break;
            }
        }
    });
    Ok(())
}

#[tauri::command]
async fn connect_server(
    app: AppHandle,
    window: WebviewWindow,
    address: String,
) -> Result<(), String> {
    if window.label() != "setup" {
        return Err("Open the server setup window first.".into());
    }
    let frontend = app.state::<Frontend>();
    let _connecting = frontend
        .connecting
        .try_lock()
        .map_err(|_| "The server is already being checked.")?;
    let address = server::parse(&address)?;
    server::verify(&app, &address).await?;
    server::save(&app, &address).map_err(|_| "Could not save the server address. Try again.")?;
    start_frontend(&app, &address).map_err(|_| "Could not open IRCord. Try again.".to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .manage(Frontend::default())
        .invoke_handler(tauri::generate_handler![connect_server])
        .setup(|app| {
            if tauri::is_dev() && app.config().build.frontend_dist.is_none() {
                let url = app
                    .config()
                    .build
                    .dev_url
                    .clone()
                    .expect("Development frontend URL");
                open_window(app.handle(), url, None, None)?;
            } else {
                match server::configured(app.handle()) {
                    Ok(Some(backend)) => {
                        if start_frontend(app.handle(), &backend).is_err() {
                            show_error(app.handle());
                        }
                    }
                    Ok(None) => setup_window(app.handle())?,
                    Err(_) => show_error(app.handle()),
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Build IRCord desktop application")
        .run(|app, event| {
            if matches!(event, RunEvent::Exit)
                && let Some(frontend) = app.try_state::<Frontend>()
                && let Some(child) = frontend.child.lock().unwrap().take()
            {
                let _ = child.kill();
            }
        });
}
