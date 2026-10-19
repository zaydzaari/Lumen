use std::path::PathBuf;

use anyhow::{Context as _, Result};
use opensubsonic::Auth;
use serde::{Deserialize, Serialize};

use crate::credentials;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Credentials {
    pub server: String,
    pub username: String,
    pub password: String,
    /// The token and salt every cover url is signed with. Made once at sign-in and kept, so a
    /// cover keeps one url across launches and the image caches can hold it.
    #[serde(default)]
    pub signature: Option<Signature>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub token: String,
    pub salt: String,
}

/// Signs once with a fresh salt, the way the server expects each request to be signed.
pub fn sign(username: &str, password: &str) -> Signature {
    let mut signature = Signature {
        token: String::new(),
        salt: String::new(),
    };
    for (key, value) in Auth::token(username, password).params() {
        match key {
            "t" => signature.token = value,
            "s" => signature.salt = value,
            _ => {}
        }
    }
    signature
}

fn path() -> PathBuf {
    credentials::dir("subsonic").join(credentials::FILE)
}

pub fn normalize_server(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("the server address is empty");
    }
    let with_scheme = match trimmed.contains("://") {
        true => trimmed.to_owned(),
        false => format!("http://{trimmed}"),
    };
    Ok(with_scheme)
}

pub fn load() -> Option<Credentials> {
    let bytes = std::fs::read(path()).ok()?;
    let mut credentials: Credentials = serde_json::from_slice(&bytes).ok()?;
    credentials.server = normalize_server(&credentials.server).ok()?;
    match credentials.username.is_empty() {
        true => None,
        false => Some(credentials),
    }
}

pub fn store(stored: &Credentials) -> Result<()> {
    let bytes =
        serde_json::to_vec_pretty(stored).context("cannot serialize subsonic credentials")?;
    credentials::write(&path(), &bytes).context("cannot store subsonic credentials")
}

pub fn forget() {
    credentials::remove(&path());
}
