use std::{fs, io, path::PathBuf};
use tauri::{AppHandle, Manager, Url};

pub fn parse(address: &str) -> Result<Url, &'static str> {
    let address = Url::parse(address.trim()).map_err(|_| "Enter a complete server address.")?;
    if !address.username().is_empty()
        || address.password().is_some()
        || address.path() != "/"
        || address.query().is_some()
        || address.fragment().is_some()
    {
        return Err("Enter the server address without a path or sign-in details.");
    }
    if !(address.scheme() == "https"
        || (address.scheme() == "http" && address.host_str() == Some("127.0.0.1")))
    {
        return Err("Use an HTTPS server address.");
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

pub async fn verify(client: &reqwest::Client, address: &Url) -> Result<(), &'static str> {
    let failed =
        "Could not find an IRCord server at this address. Check the address and try again.";
    let session = client
        .get(address.join("/api/v1/auth/session").map_err(|_| failed)?)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .map_err(|_| failed)?;
    if !session.status().is_success() {
        return Err(failed);
    }
    let session = crate::transport::read_body(session, 32 << 10)
        .await
        .map_err(|_| failed)?;
    let session: serde_json::Value = serde_json::from_slice(&session).map_err(|_| failed)?;
    if !matches!(session.get("account"), Some(serde_json::Value::Null)) {
        return Err(failed);
    }
    let model = client
        .head(
            address
                .join("/api/v1/app/speech/zonos3/v1/models/backbone/shard-000.bin")
                .map_err(|_| failed)?,
        )
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .map_err(|_| failed)?;
    if model.status() != reqwest::StatusCode::FOUND {
        return Err(
            "This server needs an IRCord backend update before the desktop app can connect.",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse;

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
    fn requires_an_origin_with_safe_transport() {
        for address in [
            "chat.ircord.com",
            "http://chat.ircord.com",
            "file:///",
            "https://name:password@example.com",
            "https://example.com/chat",
            "https://example.com?access=key",
            "https://example.com/#here",
        ] {
            assert!(parse(address).is_err());
        }
    }
}
