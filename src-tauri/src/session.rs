use reqwest::header;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};
use tauri::webview::cookie::{Cookie, time::OffsetDateTime};

const COOKIE_NAME: &str = "ircord_session";

// The native transport owns the HTTP-only credential. The WebView only sees account data.
pub struct SessionStore {
    path: PathBuf,
    session: Option<Session>,
}

#[derive(Serialize, Deserialize)]
struct Session {
    value: String,
    expires: Option<i64>,
}

impl Session {
    fn valid(&self, now: i64) -> bool {
        !self.value.is_empty()
            && self.value.len() <= 1024
            && self
                .value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
            && self.expires.is_none_or(|expiration| expiration > now)
    }
}

impl SessionStore {
    pub fn open(path: PathBuf) -> Self {
        let session = fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Session>(&raw).ok())
            .filter(|session| session.expires.is_some() && session.valid(now()));
        Self { path, session }
    }

    pub fn header(&self) -> Option<header::HeaderValue> {
        self.session
            .as_ref()
            .filter(|session| session.valid(now()))
            .and_then(|session| {
                let mut value =
                    header::HeaderValue::from_str(&format!("{COOKIE_NAME}={}", session.value))
                        .ok()?;
                value.set_sensitive(true);
                Some(value)
            })
    }

    pub fn update(&mut self, headers: &header::HeaderMap) -> io::Result<()> {
        for value in headers.get_all(header::SET_COOKIE) {
            let Some(cookie) = value
                .to_str()
                .ok()
                .and_then(|value| Cookie::parse(value).ok())
                .filter(|cookie| cookie.name() == COOKIE_NAME)
            else {
                continue;
            };
            let current = now();
            let session = Session {
                value: cookie.value().to_owned(),
                expires: cookie
                    .max_age()
                    .map(|age| current.saturating_add(age.whole_seconds()))
                    .or_else(|| cookie.expires_datetime().map(|time| time.unix_timestamp())),
            };
            self.session = session.valid(current).then_some(session);
            self.save()?;
        }
        Ok(())
    }

    fn save(&self) -> io::Result<()> {
        if let Some(session) = self
            .session
            .as_ref()
            .filter(|session| session.expires.is_some())
        {
            let parent = self.path.parent().expect("Session file has a directory");
            fs::create_dir_all(parent)?;
            // NamedTempFile uses owner-only permissions on Unix and the per-user app
            // directory's permissions on Windows. Rename makes restart recovery atomic.
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer(&mut file, session).map_err(io::Error::other)?;
            file.as_file().sync_all()?;
            file.persist(&self.path).map_err(|error| error.error)?;
        } else if let Err(error) = fs::remove_file(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(error);
        }
        Ok(())
    }
}

fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_session_survives_restart_and_logout_clears_it() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("session");
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::SET_COOKIE,
            "ircord_session=test.cookie; Path=/; HttpOnly; Max-Age=600"
                .parse()
                .unwrap(),
        );
        SessionStore::open(path.clone()).update(&headers).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let mut restarted = SessionStore::open(path.clone());
        assert_eq!(restarted.header().unwrap(), "ircord_session=test.cookie");
        headers.insert(
            header::SET_COOKIE,
            "ircord_session=; Path=/; HttpOnly; Max-Age=0"
                .parse()
                .unwrap(),
        );
        restarted.update(&headers).unwrap();
        assert!(SessionStore::open(path).header().is_none());
    }
}
