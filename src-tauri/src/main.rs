#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod notifications;
mod server;
mod session;
mod transport;

use sha2::{Digest, Sha256};
use tauri::{
    AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    webview::NewWindowResponse,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;
use transport::Transport;

const TITLE_BAR: &str = if cfg!(target_os = "macos") {
    "macos"
} else if cfg!(target_os = "windows") {
    "windows"
} else {
    "linux"
};

fn setup_window(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(
        app,
        "setup",
        WebviewUrl::App("desktop-setup/index.html".into()),
    )
    .title("IRCord")
    .inner_size(560., 440.)
    .min_inner_size(360., 350.)
    .center()
    .build()?;
    Ok(())
}

fn open_window(app: &AppHandle, server: &Url) -> tauri::Result<()> {
    let origin = server.origin().ascii_serialization();
    let marker = serde_json::json!({ "serverOrigin": origin, "titleBar": TITLE_BAR });
    let desktop_marker = format!(
        "Object.defineProperty(window, '__IRCordDesktop', {{ value: Object.freeze({marker}) }});"
    );
    let open = app.clone();
    let dev_origin = tauri::is_dev()
        .then(|| app.config().build.dev_url.as_ref().map(Url::origin))
        .flatten();
    let mut window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("IRCord")
        .inner_size(1280., 840.)
        .min_inner_size(380., 350.)
        .initialization_script(desktop_marker)
        .disable_drag_drop_handler()
        .on_navigation(move |url| {
            (url.host_str() == Some("localhost") && url.scheme() == "tauri")
                || (url.host_str() == Some("tauri.localhost")
                    && matches!(url.scheme(), "http" | "https"))
                || dev_origin
                    .as_ref()
                    .is_some_and(|origin| *origin == url.origin())
                || url.as_str() == "about:blank"
        })
        .on_new_window(move |url, _| {
            if matches!(url.scheme(), "http" | "https" | "mailto") {
                let _ = open.opener().open_url(url.as_str(), None::<&str>);
            }
            NewWindowResponse::Deny
        });
    let fingerprint = Sha256::digest(origin.as_bytes());
    #[cfg(target_os = "macos")]
    {
        window = window
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .traffic_light_position(tauri::LogicalPosition::new(15., 16.));
        let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
        window = if version.majorVersion >= 14 {
            window.data_store_identifier(fingerprint[..16].try_into().unwrap())
        } else {
            window.incognito(true)
        };
    }
    #[cfg(not(target_os = "macos"))]
    {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let fingerprint = URL_SAFE_NO_PAD.encode(fingerprint);
        let directory = app
            .path()
            .app_local_data_dir()?
            .join(format!("server-{fingerprint}"));
        window = window.decorations(false).data_directory(directory);
    }
    window.build()?;
    Ok(())
}

fn show_error(app: &AppHandle) {
    let exit = app.clone();
    app.dialog()
        .message("IRCord could not open its window. Quit IRCord and open it again.")
        .title("IRCord")
        .kind(MessageDialogKind::Error)
        .show(move |_| exit.exit(1));
}

#[derive(serde::Serialize)]
struct ServerPreview {
    origin: String,
    registration: server::Registration,
}

#[tauri::command]
async fn check_server(
    app: AppHandle,
    window: WebviewWindow,
    address: String,
) -> Result<ServerPreview, String> {
    if window.label() != "setup" {
        return Err("Open the server setup window first.".into());
    }
    let transport = app.state::<Transport>();
    let _connecting = transport
        .connecting
        .try_lock()
        .map_err(|_| "The server is already being checked.")?;
    let address = server::parse(&address)?;
    let registration = server::verify(&transport.client, &address).await?;
    Ok(ServerPreview {
        origin: address.origin().ascii_serialization(),
        registration,
    })
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
    let transport = app.state::<Transport>();
    let _connecting = transport
        .connecting
        .try_lock()
        .map_err(|_| "The server is already being checked.")?;
    let address = server::parse(&address)?;
    server::verify(&transport.client, &address).await?;
    server::save(&app, &address).map_err(|_| "Could not save the server address. Try again.")?;
    transport
        .select(&app, address.clone())
        .map_err(|_| "Could not open the IRCord session.")?;
    open_window(&app, &address).map_err(|_| "Could not open IRCord. Try again.")?;
    let _ = window.close();
    Ok(())
}

#[tauri::command]
async fn choose_server(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Open the IRCord workspace first.".into());
    }
    setup_window(&app).map_err(|_| "Could not open the server chooser.")?;
    window
        .destroy()
        .map_err(|_| "Could not close the IRCord window.".into())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Transport::new().expect("Create the native IRCord client"))
        .invoke_handler(tauri::generate_handler![
            check_server,
            connect_server,
            choose_server,
            transport::api_request,
            transport::cancel_request,
            transport::open_socket,
            transport::send_socket,
            transport::close_socket,
            notifications::notification_permission,
            notifications::request_notification_permission,
            notifications::show_notification,
            notifications::focus_notification_window,
        ])
        .setup(|app| {
            notifications::listen(app.handle())?;
            match server::configured(app.handle()) {
                Ok(Some(server)) => {
                    app.state::<Transport>()
                        .select(app.handle(), server.clone())?;
                    if open_window(app.handle(), &server).is_err() {
                        show_error(app.handle());
                    }
                }
                Ok(None) => setup_window(app.handle())?,
                Err(_) => show_error(app.handle()),
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Run IRCord Desktop");
}
