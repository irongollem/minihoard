//! MyMiniFactory REST API client and data models.
//!
//! Two auth modes are supported, mirroring MMF's two methods:
//! - [`Client::with_bearer`] — OAuth access token (required for download URLs)
//! - [`Client::with_api_key`] — an API key sent as the `?key=` query param
//!   (read access; download URLs are documented as unavailable this way)
//!
//! The exact "your library" listing endpoint is what the M0 spike pins down,
//! so the client exposes a generic [`Client::get`] returning raw JSON for
//! exploration alongside typed helpers.

use serde::Deserialize;

use crate::error::{Error, Result};

pub const API_BASE: &str = "https://www.myminifactory.com/api/v2";

enum Auth {
    Bearer(String),
    ApiKey(String),
}

/// MyMiniFactory API client.
pub struct Client {
    http: reqwest::Client,
    base: String,
    auth: Auth,
}

/// A single downloadable file within an object.
#[derive(Debug, Clone, Deserialize)]
pub struct File {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: Option<u64>,
    /// OAuth-only: direct download URL (absent when using an API key).
    #[serde(default)]
    pub download_url: Option<String>,
}

impl Client {
    /// Build a client authenticated with an OAuth bearer access token.
    pub fn with_bearer(access_token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: API_BASE.to_string(),
            auth: Auth::Bearer(access_token.into()),
        }
    }

    /// Build a client authenticated with an API key (`?key=` query param).
    pub fn with_api_key(key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: API_BASE.to_string(),
            auth: Auth::ApiKey(key.into()),
        }
    }

    /// The bearer access token, if this client uses one (needed to authenticate
    /// downloads against the download host).
    pub fn bearer_token(&self) -> Option<&str> {
        match &self.auth {
            Auth::Bearer(t) => Some(t),
            Auth::ApiKey(_) => None,
        }
    }

    /// Kept for the `whoami` proof: fetch the authenticated user's profile.
    pub async fn current_user(&self) -> Result<serde_json::Value> {
        self.get("/user").await
    }

    /// GET an API path (relative to the base, e.g. `/user`) and return raw JSON.
    pub async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.base, path);
        let mut req = self.http.get(&url);
        req = match &self.auth {
            Auth::Bearer(token) => req.bearer_auth(token),
            Auth::ApiKey(key) => req.query(&[("key", key)]),
        };

        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(Error::Api {
                status: status.as_u16(),
                message,
            });
        }
        Ok(resp.json().await?)
    }
}
