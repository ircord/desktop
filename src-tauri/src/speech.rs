use std::net::IpAddr;

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::header;
use serde::{Deserialize, Serialize};
use tauri::{State, Url, WebviewWindow};

use crate::transport::{Transport, read_body};

const MAX_AUDIO: usize = 4 << 20;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechRequest {
    id: String,
    base_url: String,
    model: String,
    voice: String,
    input: String,
    key: String,
}

#[derive(Serialize)]
pub struct SpeechResponse {
    status: u16,
    audio: String,
}

fn speech_url(base: &str) -> Result<Url, &'static str> {
    const INVALID: &str = "Enter a valid speech service address.";
    if base.len() > 500
        || base.chars().any(|c| {
            c.is_whitespace() || matches!(c, ';' | ',' | '\'' | '"' | '\\' | '*' | '?' | '#')
        })
    {
        return Err(INVALID);
    }
    let mut url = Url::parse(base).map_err(|_| INVALID)?;
    let host = url.host_str().ok_or(INVALID)?;
    let loopback = host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(INVALID);
    }
    url.path_segments_mut()
        .map_err(|_| INVALID)?
        .pop_if_empty()
        .extend(["audio", "speech"]);
    Ok(url)
}

#[tauri::command]
pub async fn speech_request(
    window: WebviewWindow,
    transport: State<'_, Transport>,
    request: SpeechRequest,
) -> Result<SpeechResponse, String> {
    let server = transport.server(&window)?;
    if transport.session(&server).is_none() {
        return Err("Sign in to use Read Aloud.".into());
    }
    let url = speech_url(&request.base_url)?;
    let short_name = |value: &str| {
        !value.is_empty()
            && value.encode_utf16().count() <= 100
            && !value.chars().any(char::is_control)
    };
    if !short_name(&request.model)
        || !short_name(&request.voice)
        || request.input.encode_utf16().count() > 1_000
        || request.key.encode_utf16().count() > 500
        || request.key.chars().any(char::is_control)
    {
        return Err("The speech request is invalid.".into());
    }
    transport
        .cancellable(request.id.clone(), request_audio(&transport, url, &request))
        .await
        .map_err(str::to_owned)
}

async fn request_audio(
    transport: &Transport,
    url: Url,
    request: &SpeechRequest,
) -> Result<SpeechResponse, &'static str> {
    // The redirect-free client sends only the speech service key to this endpoint.
    let mut upstream = transport
        .client
        .post(url)
        .header(header::ACCEPT, "audio/mpeg")
        .json(&serde_json::json!({
            "model": request.model,
            "voice": request.voice,
            "input": request.input,
            "response_format": "mp3",
        }));
    if !request.key.is_empty() {
        upstream = upstream.bearer_auth(&request.key);
    }
    let response = upstream
        .send()
        .await
        .map_err(|_| "Could not reach the speech service.")?;
    let status = response.status().as_u16();
    let audio = if response.status().is_success() {
        STANDARD.encode(read_body(response, MAX_AUDIO).await?)
    } else {
        String::new()
    };
    Ok(SpeechResponse { status, audio })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    use super::{SpeechRequest, Transport, request_audio, speech_url};

    #[test]
    fn speech_url_keeps_the_service_path_and_limits_http_to_loopback() {
        for (base, endpoint) in [
            (
                "https://example.test/v1/",
                "https://example.test/v1/audio/speech",
            ),
            (
                "http://localhost:8090",
                "http://localhost:8090/audio/speech",
            ),
            ("http://[::1]:8090/v1", "http://[::1]:8090/v1/audio/speech"),
        ] {
            assert_eq!(speech_url(base).unwrap().as_str(), endpoint);
        }
        for base in [
            "http://example.test/v1",
            "https://user:password@example.test/v1",
            "https://example.test/v1?token=x",
            "https://example.test/v1#fragment",
            "file:///tmp/speech",
        ] {
            assert!(speech_url(base).is_err());
        }
    }

    #[tokio::test]
    async fn speech_requests_send_only_the_service_key_and_do_not_follow_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&mut stream);
            let headers: Vec<String> = (&mut reader)
                .lines()
                .map(Result::unwrap)
                .take_while(|line| !line.is_empty())
                .collect();
            assert_eq!(headers[0], "POST /v1/audio/speech HTTP/1.1");
            assert!(
                headers
                    .iter()
                    .any(|line| line.eq_ignore_ascii_case("authorization: Bearer fixture-key"))
            );
            assert!(
                !headers
                    .iter()
                    .any(|line| line.to_ascii_lowercase().starts_with("cookie:"))
            );
            let length = headers
                .iter()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")?
                        .parse()
                        .ok()
                })
                .unwrap();
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["response_format"], "mp3");
            assert_eq!(body["input"], "Test");
            write!(
                stream,
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: /elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        });
        let request = SpeechRequest {
            id: "fixture".into(),
            base_url: format!("http://{address}/v1"),
            model: "fixture-model".into(),
            voice: "fixture-voice".into(),
            input: "Test".into(),
            key: "fixture-key".into(),
        };
        let transport = Transport::new().unwrap();
        let response = request_audio(&transport, speech_url(&request.base_url).unwrap(), &request)
            .await
            .unwrap();
        assert_eq!(response.status, 307);
        assert!(response.audio.is_empty());
        server.join().unwrap();
    }
}
