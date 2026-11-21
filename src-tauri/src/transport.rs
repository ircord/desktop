use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, Method, header};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Mutex, RwLock},
    time::Duration,
};
use tauri::{AppHandle, Manager, State, Url, WebviewWindow, ipc::Channel, webview::cookie::Cookie};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

const SESSION: &str = "ircord_session";
const MAX_BODY: usize = 32 << 20;
const MAX_EVENT: usize = 8 << 20;
const MAX_COMMAND: usize = 16 << 10;
const OFFLINE: &str = "Could not reach the IRCord server.";

pub struct Transport {
    pub client: Client,
    pub connecting: tokio::sync::Mutex<()>,
    server: RwLock<Option<Url>>,
    requests: Mutex<HashMap<String, oneshot::Sender<()>>>,
    sockets: Mutex<HashMap<String, mpsc::Sender<SocketCommand>>>,
}

impl Transport {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(60))
                .build()?,
            connecting: tokio::sync::Mutex::new(()),
            server: RwLock::new(None),
            requests: Mutex::new(HashMap::new()),
            sockets: Mutex::new(HashMap::new()),
        })
    }

    pub fn select(&self, server: Url) {
        *self.server.write().unwrap() = Some(server);
    }

    fn server(&self, window: &WebviewWindow) -> Result<Url, &'static str> {
        if window.label() != "main" {
            return Err("Open the IRCord workspace first.");
        }
        self.server
            .read()
            .unwrap()
            .clone()
            .ok_or("Select an IRCord server first.")
    }
}

fn target(server: &Url, path: &str) -> Result<Url, &'static str> {
    if !path.starts_with('/') || path.len() > 16_384 {
        return Err("Invalid IRCord request.");
    }
    let target = server.join(path).map_err(|_| "Invalid IRCord request.")?;
    if target.origin() != server.origin()
        || target.fragment().is_some()
        || !(target.path() == "/openapi.json"
            || ["/api/v1/", "/files/", "/emotes/"]
                .iter()
                .any(|prefix| target.path().starts_with(prefix)))
    {
        return Err("Invalid IRCord request.");
    }
    Ok(target)
}

fn session(window: &WebviewWindow, url: &Url) -> Result<Option<header::HeaderValue>, &'static str> {
    let cookies = window
        .cookies_for_url(url.clone())
        .map_err(|_| "Could not read the IRCord session.")?;
    cookies
        .iter()
        .find(|cookie| cookie.name() == SESSION)
        .map(|cookie| {
            header::HeaderValue::from_str(&format!("{SESSION}={}", cookie.value()))
                .map_err(|_| "Could not read the IRCord session.")
        })
        .transpose()
}

fn store_session(
    window: &WebviewWindow,
    server: &Url,
    headers: &header::HeaderMap,
) -> Result<(), &'static str> {
    for value in headers.get_all(header::SET_COOKIE) {
        let Some(mut cookie) = value
            .to_str()
            .ok()
            .and_then(|value| Cookie::parse(value.to_owned()).ok())
            .filter(|cookie| cookie.name() == SESSION)
        else {
            continue;
        };
        cookie.set_domain(
            server
                .host_str()
                .ok_or("Invalid IRCord server.")?
                .to_owned(),
        );
        cookie.set_path("/");
        cookie.set_http_only(true);
        cookie.set_secure(server.scheme() == "https");
        window
            .set_cookie(cookie)
            .map_err(|_| "Could not save the IRCord session.")?;
    }
    Ok(())
}

pub async fn read_body(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, &'static str> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err("The IRCord response is too large.");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| OFFLINE)? {
        if body.len() + chunk.len() > limit {
            return Err("The IRCord response is too large.");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Deserialize)]
pub struct ApiRequest {
    id: String,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    credentials: bool,
}

#[derive(Serialize)]
pub struct ApiResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

#[tauri::command]
pub async fn api_request(
    window: WebviewWindow,
    transport: State<'_, Transport>,
    request: ApiRequest,
) -> Result<ApiResponse, String> {
    let server = transport.server(&window)?;
    let url = target(&server, &request.path)?;
    let method =
        Method::from_bytes(request.method.as_bytes()).map_err(|_| "Invalid IRCord request.")?;
    if !matches!(
        method,
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return Err("Invalid IRCord request.".into());
    }
    let mut upstream = transport.client.request(method, url.clone());
    for (name, value) in request.headers {
        if matches!(
            name.as_str(),
            "accept"
                | "content-type"
                | "range"
                | "if-none-match"
                | "if-modified-since"
                | "x-registration-key"
        ) {
            upstream = upstream.header(name, value);
        }
    }
    if request.credentials
        && let Some(value) = session(&window, &url)?
    {
        upstream = upstream.header(header::COOKIE, value);
    }
    if let Some(encoded) = request.body {
        if encoded.len() > MAX_BODY.div_ceil(3) * 4 {
            return Err("The IRCord request is too large.".into());
        }
        let body = STANDARD
            .decode(encoded)
            .map_err(|_| "Invalid IRCord request.")?;
        if body.len() > MAX_BODY {
            return Err("The IRCord request is too large.".into());
        }
        upstream = upstream.body(body);
    }
    let (cancel, cancelled) = oneshot::channel();
    transport
        .requests
        .lock()
        .unwrap()
        .insert(request.id.clone(), cancel);
    let result = tokio::select! {
        _ = cancelled => Err("The IRCord request was cancelled."),
        result = async {
            let response = upstream.send().await.map_err(|_| OFFLINE)?;
            if request.credentials {
                store_session(&window, &server, response.headers())?;
            }
            let status = response.status().as_u16();
            let headers = response.headers().iter().filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "content-type" | "content-length" | "content-range" | "content-disposition" |
                    "cache-control" | "etag" | "last-modified" | "location" | "retry-after"
                )
            }).filter_map(|(key, value)| {
                value.to_str().ok().map(|value| (key.to_string(), value.to_owned()))
            }).collect();
            let bytes = read_body(response, MAX_BODY).await?;
            Ok(ApiResponse { status, headers, body: STANDARD.encode(bytes) })
        } => result,
    };
    transport.requests.lock().unwrap().remove(&request.id);
    result.map_err(str::to_owned)
}

#[tauri::command]
pub fn cancel_request(transport: State<'_, Transport>, id: String) {
    if let Some(cancel) = transport.requests.lock().unwrap().remove(&id) {
        let _ = cancel.send(());
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SocketEvent {
    Open,
    Message { data: String },
    Close { code: u16, reason: String },
    Error,
}

enum SocketCommand {
    Send(String),
    Close,
}

#[tauri::command]
pub async fn open_socket(
    app: AppHandle,
    window: WebviewWindow,
    transport: State<'_, Transport>,
    id: String,
    path: String,
    events: Channel<SocketEvent>,
) -> Result<(), String> {
    let server = transport.server(&window)?;
    let mut url = target(&server, &path)?;
    if url.path() != "/api/v1/events" {
        return Err("Invalid IRCord event stream.".into());
    }
    let cookie = session(&window, &url)?.ok_or("Sign in to open the IRCord event stream.")?;
    url.set_scheme(if server.scheme() == "https" {
        "wss"
    } else {
        "ws"
    })
    .map_err(|_| "Invalid IRCord event stream.")?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| "Invalid IRCord event stream.")?;
    request.headers_mut().insert(header::COOKIE, cookie);
    let (queue, commands) = mpsc::channel(32);
    transport.sockets.lock().unwrap().insert(id.clone(), queue);
    tauri::async_runtime::spawn(async move {
        run_socket(request, commands, events).await;
        app.state::<Transport>().sockets.lock().unwrap().remove(&id);
    });
    Ok(())
}

async fn run_socket(
    request: tokio_tungstenite::tungstenite::handshake::client::Request,
    mut commands: mpsc::Receiver<SocketCommand>,
    events: Channel<SocketEvent>,
) {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_EVENT))
        .max_frame_size(Some(MAX_EVENT));
    let connected = tokio::select! {
        _ = commands.recv() => return,
        result = tokio::time::timeout(Duration::from_secs(10), connect_async_with_config(request, Some(config), false)) => result,
    };
    let Ok(Ok((mut socket, _))) = connected else {
        let _ = events.send(SocketEvent::Error);
        let _ = events.send(SocketEvent::Close {
            code: 1006,
            reason: String::new(),
        });
        return;
    };
    if events.send(SocketEvent::Open).is_err() {
        return;
    }
    let (code, reason) = loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(SocketCommand::Send(data)) => {
                    if socket.send(Message::Text(data.into())).await.is_err() {
                        let _ = events.send(SocketEvent::Error);
                        break (1006, String::new());
                    }
                }
                _ => {
                    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await;
                    break (1000, String::new());
                }
            },
            result = socket.next() => match result {
                Some(Ok(Message::Text(data))) => {
                    if events.send(SocketEvent::Message { data: data.to_string() }).is_err() {
                        return;
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    break frame.map(|frame| (frame.code.into(), frame.reason.to_string()))
                        .unwrap_or((1005, String::new()));
                }
                Some(Ok(_)) => {}
                _ => {
                    let _ = events.send(SocketEvent::Error);
                    break (1006, String::new());
                }
            }
        }
    };
    let _ = events.send(SocketEvent::Close { code, reason });
}

#[tauri::command]
pub fn send_socket(
    transport: State<'_, Transport>,
    id: String,
    data: String,
) -> Result<(), String> {
    if data.len() > MAX_COMMAND {
        return Err("The IRCord event is too large.".into());
    }
    transport
        .sockets
        .lock()
        .unwrap()
        .get(&id)
        .ok_or("The IRCord event stream is closed.")?
        .try_send(SocketCommand::Send(data))
        .map_err(|_| "Could not send the IRCord event.".into())
}

#[tauri::command]
pub fn close_socket(transport: State<'_, Transport>, id: String) {
    if let Some(queue) = transport.sockets.lock().unwrap().remove(&id) {
        let _ = queue.try_send(SocketCommand::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_stay_on_the_selected_server() {
        let server = Url::parse("https://ircord.example:8443").unwrap();
        assert!(target(&server, "/api/v1/app/preferences").is_ok());
        assert!(target(&server, "/files/id/token/image.png").is_ok());
        assert!(target(&server, "//other.example/api/v1/auth/session").is_err());
        assert!(target(&server, "/api/v1/../../admin").is_err());
        assert!(target(&server, "/api/v1/auth/session#fragment").is_err());
    }
}
