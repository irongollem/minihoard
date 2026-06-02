//! Bootstrap the MyMiniFactory website session cookie from a browser you're
//! already logged into — so you don't have to copy it from DevTools (the
//! `set-cookie` flow). Reads the browser's own cookie store (à la
//! `yt-dlp --cookies-from-browser`); no password, no scripted login.

use std::collections::BTreeMap;

use rookie::common::enums::Cookie;

use crate::error::{Error, Result};

const DOMAIN: &str = "myminifactory.com";

/// Browsers we can read cookies from (case-insensitive names accepted on the CLI).
const SUPPORTED: &[&str] = &[
    "chrome", "firefox", "edge", "brave", "safari", "arc", "chromium", "vivaldi", "opera",
];

/// Read `myminifactory.com` cookies from `browser` (or auto-detect across all
/// installed browsers when `None`) and assemble a `Cookie:` header value.
pub fn import_from_browser(browser: Option<&str>) -> Result<String> {
    let domains = Some(vec![DOMAIN.to_string()]);
    let cookies = read(browser, domains)?;

    if cookies.is_empty() {
        let where_ = browser
            .map(|b| format!(" in {b}"))
            .unwrap_or_else(|| " in any detected browser".into());
        return Err(Error::Auth(format!(
            "no {DOMAIN} cookies found{where_} — log in to MyMiniFactory there first, \
             then re-run (or paste manually with `minihoard set-cookie`)"
        )));
    }

    // Build the header from name=value pairs; dedupe by name (last write wins).
    let mut pairs: BTreeMap<String, String> = BTreeMap::new();
    for c in cookies {
        pairs.insert(c.name, c.value);
    }
    let header = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    Ok(header)
}

fn read(browser: Option<&str>, domains: Option<Vec<String>>) -> Result<Vec<Cookie>> {
    let result = match browser.map(|b| b.to_ascii_lowercase()).as_deref() {
        None => rookie::load(domains),
        Some("chrome") => rookie::chrome(domains),
        Some("chromium") => rookie::chromium(domains),
        Some("firefox") => rookie::firefox(domains),
        Some("edge") => rookie::edge(domains),
        Some("brave") => rookie::brave(domains),
        Some("arc") => rookie::arc(domains),
        #[cfg(target_os = "macos")]
        Some("safari") => rookie::safari(domains),
        #[cfg(not(target_os = "macos"))]
        Some("safari") => {
            return Err(Error::Auth("Safari cookies are only available on macOS".into()))
        }
        Some("vivaldi") => rookie::vivaldi(domains),
        Some("opera") => rookie::opera(domains),
        Some(other) => {
            return Err(Error::Auth(format!(
                "unknown browser `{other}` (supported: {})",
                SUPPORTED.join(", ")
            )))
        }
    };
    result.map_err(|e| Error::Auth(format!("reading browser cookies: {e}")))
}
