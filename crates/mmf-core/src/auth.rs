//! OAuth2 authentication against MyMiniFactory.
//!
//! Flow: Authorization Code grant via a loopback redirect. [`login`] opens the
//! browser, a tiny local HTTP server catches the redirect, we exchange the code
//! at the token endpoint (HTTP Basic auth with client_id/secret), and store the
//! resulting tokens. [`access_token`] returns a cached access token while it's
//! valid, otherwise silently refreshes.
//!
//! All secrets live in a SINGLE OS-keychain item ([`Credentials`] as JSON) so a
//! normal run triggers at most one keychain prompt.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{Error, Result};

pub const AUTHORIZE_URL: &str = "https://auth.myminifactory.com/web/authorize";
pub const TOKEN_URL: &str = "https://auth.myminifactory.com/v1/oauth/tokens";

const KEYRING_SERVICE: &str = "nl.crocode.minihoard";
const KEYRING_USER: &str = "credentials";

/// Everything secret, stored as one keychain item.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Credentials {
    /// Confidential-client secret (from the MMF app's API key).
    client_secret: Option<String>,
    /// Long-lived refresh token (code grant only).
    refresh_token: Option<String>,
    /// Cached access token.
    access_token: Option<String>,
    /// Unix seconds at which the access token expires (with a safety margin).
    expires_at: Option<u64>,
    /// Raw `Cookie:` header copied from a logged-in browser, used for the
    /// website-only library-listing endpoint (which ignores OAuth).
    session_cookie: Option<String>,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The token endpoint response (RFC 6749 §5.1). Extra fields are ignored.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// Keychain-backed credential store. One item, loaded/saved as a whole.
pub struct TokenStore;

impl TokenStore {
    fn entry() -> Result<keyring::Entry> {
        Ok(keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)?)
    }

    fn load() -> Result<Credentials> {
        match Self::entry()?.get_password() {
            Ok(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
            Err(keyring::Error::NoEntry) => Ok(Credentials::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn save(creds: &Credentials) -> Result<()> {
        let json = serde_json::to_string(creds)?;
        Self::entry()?.set_password(&json)?;
        Ok(())
    }

    pub fn save_client_secret(secret: &str) -> Result<()> {
        let mut creds = Self::load()?;
        creds.client_secret = Some(secret.to_string());
        Self::save(&creds)
    }

    pub fn client_secret() -> Result<Option<String>> {
        Ok(Self::load()?.client_secret)
    }

    pub fn save_session_cookie(cookie: &str) -> Result<()> {
        let mut creds = Self::load()?;
        creds.session_cookie = Some(cookie.to_string());
        Self::save(&creds)
    }

    pub fn session_cookie() -> Result<Option<String>> {
        Ok(Self::load()?.session_cookie)
    }

    /// Forget session tokens (keeps the client secret — that's config).
    pub fn clear() -> Result<()> {
        let mut creds = Self::load()?;
        creds.refresh_token = None;
        creds.access_token = None;
        creds.expires_at = None;
        Self::save(&creds)
    }

    pub fn is_logged_in() -> bool {
        match Self::load() {
            Ok(c) => c.refresh_token.is_some() || c.access_token.is_some(),
            Err(_) => false,
        }
    }
}

/// Run the interactive browser login (Authorization Code grant).
pub async fn login(client_id: &str, redirect_port: u16) -> Result<()> {
    let redirect_uri = format!("http://localhost:{redirect_port}/callback");
    let state = uuid::Uuid::new_v4().to_string();

    let authorize_url = Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", client_id),
            ("redirect_uri", &redirect_uri),
            ("response_type", "code"),
            ("state", &state),
        ],
    )
    .map_err(|e| Error::Auth(format!("building authorize url: {e}")))?;

    let server = tiny_http::Server::http(("127.0.0.1", redirect_port))
        .map_err(|e| Error::Auth(format!("starting loopback server on :{redirect_port}: {e}")))?;

    println!("Opening your browser to authorize minihoard...");
    println!("If it doesn't open, paste this URL:\n  {authorize_url}\n");
    let _ = open::that(authorize_url.as_str());

    let expected_state = state.clone();
    let code = tokio::task::spawn_blocking(move || wait_for_code(server, &expected_state))
        .await
        .map_err(|e| Error::Auth(format!("loopback task: {e}")))??;

    let token = exchange_code(client_id, &code, &redirect_uri).await?;
    let Some(refresh) = token.refresh_token.clone() else {
        return Err(Error::Auth(
            "token response had no refresh_token; cannot persist a durable session".into(),
        ));
    };

    let mut creds = TokenStore::load()?;
    creds.refresh_token = Some(refresh);
    creds.access_token = Some(token.access_token);
    creds.expires_at = Some(now_unix() + token.expires_in.unwrap_or(3600).saturating_sub(60));
    TokenStore::save(&creds)?;
    Ok(())
}

/// Block until the OAuth redirect arrives, validate `state`, and return `code`.
fn wait_for_code(server: tiny_http::Server, expected_state: &str) -> Result<String> {
    loop {
        let request = server
            .recv()
            .map_err(|e| Error::Auth(format!("receiving redirect: {e}")))?;

        let full = format!("http://localhost{}", request.url());
        let parsed = Url::parse(&full).map_err(|e| Error::Auth(format!("parsing redirect: {e}")))?;

        if parsed.path() != "/callback" {
            let _ = request.respond(tiny_http::Response::empty(404));
            continue;
        }

        let params: HashMap<_, _> = parsed.query_pairs().into_owned().collect();

        if let Some(err) = params.get("error") {
            respond_html(request, "Authorization failed. You can close this tab.");
            return Err(Error::Auth(format!("authorization denied: {err}")));
        }
        let state = params.get("state").map(String::as_str).unwrap_or_default();
        if state != expected_state {
            respond_html(request, "State mismatch. You can close this tab.");
            return Err(Error::Auth("state mismatch (possible CSRF)".into()));
        }
        match params.get("code") {
            Some(code) => {
                respond_html(request, "minihoard is authorized. You can close this tab.");
                return Ok(code.clone());
            }
            None => {
                respond_html(request, "No authorization code received. You can close this tab.");
                return Err(Error::Auth("redirect missing `code`".into()));
            }
        }
    }
}

fn respond_html(request: tiny_http::Request, body: &str) {
    let html = format!("<!doctype html><meta charset=utf-8><title>minihoard</title><body style='font-family:system-ui;padding:3rem'><h2>{body}</h2>");
    let header = "Content-Type: text/html; charset=utf-8".parse::<tiny_http::Header>();
    let mut response = tiny_http::Response::from_string(html);
    if let Ok(h) = header {
        response.add_header(h);
    }
    let _ = request.respond(response);
}

/// Exchange an authorization code for tokens.
async fn exchange_code(client_id: &str, code: &str, redirect_uri: &str) -> Result<TokenResponse> {
    let form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
    ];
    post_token(client_id, form).await
}

/// Return a usable access token, refreshing (and re-caching) if expired.
pub async fn access_token(client_id: &str) -> Result<String> {
    let mut creds = TokenStore::load()?;

    if let (Some(token), Some(exp)) = (&creds.access_token, creds.expires_at) {
        if exp > now_unix() {
            return Ok(token.clone());
        }
    }

    let refresh = creds.refresh_token.clone().ok_or(Error::NotAuthenticated)?;
    let token = post_token(
        client_id,
        vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh),
        ],
    )
    .await?;

    creds.access_token = Some(token.access_token.clone());
    creds.expires_at = Some(now_unix() + token.expires_in.unwrap_or(3600).saturating_sub(60));
    if let Some(new_refresh) = token.refresh_token {
        creds.refresh_token = Some(new_refresh);
    }
    TokenStore::save(&creds)?;
    Ok(token.access_token)
}

/// POST to the token endpoint. `client_id`/`client_secret` are sent as HTTP
/// Basic Auth credentials (per MMF docs), not in the form body.
async fn post_token(client_id: &str, form: Vec<(&str, String)>) -> Result<TokenResponse> {
    let secret = TokenStore::client_secret()?.ok_or_else(|| {
        Error::Auth(
            "no client secret stored — re-run `minihoard configure` and paste the secret \
             from your MMF app's API key"
                .into(),
        )
    })?;

    let resp = reqwest::Client::new()
        .post(TOKEN_URL)
        .basic_auth(client_id, Some(secret))
        .form(&form)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(Error::Auth(format!(
            "token endpoint returned {}: {body}",
            status.as_u16()
        )));
    }
    serde_json::from_str(&body)
        .map_err(|e| Error::Auth(format!("could not parse token response ({e}); body was: {body}")))
}
