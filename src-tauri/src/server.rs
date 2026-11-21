use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{fs, io, path::PathBuf, time::Duration};
use tauri::{AppHandle, Manager, Url};

pub fn parse(address: &str) -> Result<Url, &'static str> {
    let address = address.trim();
    let parsed = if address.contains("://") {
        Url::parse(address)
    } else {
        Url::parse(&format!("https://{address}"))
    };
    let mut address = parsed.map_err(|_| "Enter a complete server address.")?;
    if !address.username().is_empty()
        || address.password().is_some()
        || address.path() != "/"
        || address.query().is_some()
        || address.fragment().is_some()
    {
        return Err("Enter the server address without a path or sign-in details.");
    }
    // Use the same native session for the two spellings of the local backend.
    if address.scheme() == "http" && address.host_str() == Some("localhost") {
        address
            .set_host(Some("127.0.0.1"))
            .map_err(|_| "Enter a complete server address.")?;
    }
    if !(address.scheme() == "https"
        || (address.scheme() == "http" && address.host_str() == Some("127.0.0.1")))
    {
        return Err("Use HTTPS, or use HTTP for a local server.");
    }
    Ok(address)
}

fn config_file(app: &AppHandle) -> tauri::Result<PathBuf> {
    let name = if hot_reload(app) {
        "server-dev"
    } else {
        "server"
    };
    Ok(app.path().app_config_dir()?.join(name))
}

fn hot_reload(app: &AppHandle) -> bool {
    tauri::is_dev() && app.config().build.dev_url.is_some()
}

pub fn configured(app: &AppHandle) -> Result<Option<Url>, Box<dyn std::error::Error>> {
    if let Ok(address) = std::env::var("IRCORD_DESKTOP_BACKEND_URL") {
        return parse(&address)
            .map(Some)
            .map_err(|error| io::Error::other(error).into());
    }
    match fs::read_to_string(config_file(app)?) {
        Ok(address) => Ok(parse(&address).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound && hot_reload(app) => {
            Ok(Some(Url::parse("http://127.0.0.1:8080")?))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn save(app: &AppHandle, address: &Url) -> Result<(), Box<dyn std::error::Error>> {
    let path = config_file(app)?;
    fs::create_dir_all(path.parent().unwrap())?;
    let staging = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&staging, address.origin().ascii_serialization())?;
    fs::rename(staging, path)?;
    Ok(())
}

const NOT_IRCORD: &str =
    "Could not verify this address as an IRCord server. Check the address and try again.";
const UNREACHABLE: &str = "Could not reach the server. Check the address and your connection.";

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Registration {
    Open,
    KeyRequired,
    Closed,
}

#[derive(Deserialize)]
struct Session {
    account: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationPolicy {
    registration_enabled: bool,
    registration_key_required: bool,
}

async fn probe<T: DeserializeOwned>(
    client: &reqwest::Client,
    address: &Url,
    path: &str,
) -> Result<T, &'static str> {
    let response = client
        .get(address.join(path).map_err(|_| NOT_IRCORD)?)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|_| UNREACHABLE)?;
    if !response.status().is_success() {
        return Err(NOT_IRCORD);
    }
    let body = crate::transport::read_body(response, 32 << 10)
        .await
        .map_err(|_| NOT_IRCORD)?;
    serde_json::from_slice(&body).map_err(|_| NOT_IRCORD)
}

pub async fn verify(client: &reqwest::Client, address: &Url) -> Result<Registration, &'static str> {
    let (session, registration) = tokio::join!(
        probe::<Session>(client, address, "/api/v1/auth/session"),
        probe::<RegistrationPolicy>(client, address, "/api/v1/auth/registration"),
    );
    if !session?.account.is_null() {
        return Err(NOT_IRCORD);
    }
    let registration = registration?;
    Ok(
        match (
            registration.registration_enabled,
            registration.registration_key_required,
        ) {
            (false, _) => Registration::Closed,
            (true, true) => Registration::KeyRequired,
            (true, false) => Registration::Open,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{NOT_IRCORD, Registration, parse, verify};
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn accepts_a_public_or_local_ircord_origin() {
        for address in [
            "https://chat.ircord.com",
            "https://myamazingsetup.com:8443/",
            "http://127.0.0.1:8080",
        ] {
            assert!(parse(address).is_ok());
        }
    }

    #[test]
    fn localhost_uses_the_same_origin_as_the_loopback_address() {
        assert_eq!(
            parse("http://localhost:8080").unwrap(),
            parse("http://127.0.0.1:8080").unwrap()
        );
    }

    #[test]
    fn bare_hostnames_use_https() {
        assert_eq!(
            parse(" chat.ircord.com ").unwrap().as_str(),
            "https://chat.ircord.com/"
        );
        assert_eq!(parse("google.com").unwrap().as_str(), "https://google.com/");
    }

    #[test]
    fn requires_an_origin_with_safe_transport() {
        for address in [
            "",
            "http://chat.ircord.com",
            "http://localhost.example.com:8080",
            "file:///",
            "https://name:password@example.com",
            "https://example.com/chat",
            "https://example.com?access=key",
            "https://example.com/#here",
        ] {
            assert!(parse(address).is_err());
        }
    }

    async fn check(
        session: &'static str,
        registration: &'static str,
    ) -> Result<Registration, &'static str> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let fixture = thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.unwrap();
                let mut lines = BufReader::new(&mut stream).lines();
                let request = lines.next().unwrap().unwrap();
                for header in lines
                    .map(Result::unwrap)
                    .take_while(|line| !line.is_empty())
                {
                    let header = header.to_ascii_lowercase();
                    assert!(
                        !header.starts_with("cookie:") && !header.starts_with("authorization:")
                    );
                }
                let body = match request.split_whitespace().nth(1) {
                    Some("/api/v1/auth/session") => session,
                    Some("/api/v1/auth/registration") => registration,
                    _ => panic!("Unexpected server probe: {request}"),
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let transport = crate::transport::Transport::new().unwrap();
        let result = verify(&transport.client, &address).await;
        fixture.join().unwrap();
        result
    }

    #[tokio::test]
    async fn probes_signed_out_auth_and_reports_registration_policy() {
        for (body, expected) in [
            (
                r#"{"registrationEnabled":true,"registrationKeyRequired":false}"#,
                Registration::Open,
            ),
            (
                r#"{"registrationEnabled":true,"registrationKeyRequired":true}"#,
                Registration::KeyRequired,
            ),
            (
                r#"{"registrationEnabled":false,"registrationKeyRequired":false}"#,
                Registration::Closed,
            ),
        ] {
            assert_eq!(check(r#"{"account":null}"#, body).await, Ok(expected));
        }
    }

    #[tokio::test]
    async fn rejects_unrelated_responses_or_missing_api_fields() {
        let valid_policy = r#"{"registrationEnabled":true,"registrationKeyRequired":false}"#;
        assert_eq!(
            check("<!doctype html>", valid_policy).await,
            Err(NOT_IRCORD)
        );
        assert_eq!(check("{}", valid_policy).await, Err(NOT_IRCORD));
        assert_eq!(check(r#"{"account":null}"#, "{}").await, Err(NOT_IRCORD));
    }
}
