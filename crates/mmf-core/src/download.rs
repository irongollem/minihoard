//! Resumable downloads with progress reporting (M5).
//!
//! The download host (`www.myminifactory.com/download/...`) is behind
//! Cloudflare, which issues a managed JS challenge to non-browser clients. We
//! use `wreq` with Chrome emulation so the TLS/HTTP2 fingerprint matches a real
//! browser and Cloudflare's passive bot-score lets us through.

use std::io::Write;
use std::path::{Path, PathBuf};

use wreq_util::Emulation;

use crate::error::{Error, Result};

/// Outcome of a single download.
#[derive(Debug, Clone)]
pub struct DownloadReport {
    pub url: String,
    pub dest: PathBuf,
    pub bytes: u64,
    pub resumed: bool,
}

fn dl_err(context: &str, e: impl std::fmt::Display) -> Error {
    Error::Download(format!("{context}: {e}"))
}

/// Download `url` to `dest`, sending a bearer token for authentication.
///
/// Resumes a partial file via an HTTP Range request when the server supports
/// it (otherwise restarts cleanly). `on_progress(downloaded, total)` is called
/// as bytes arrive — `total` is the full expected size when known.
pub async fn download_to(
    url: &str,
    dest: &Path,
    bearer_token: &str,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<DownloadReport> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Resume support: if a partial file exists, ask the server to continue.
    let mut existing = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);

    let client = wreq::Client::builder()
        .emulation(Emulation::Chrome137)
        .redirect(wreq::redirect::Policy::limited(10))
        .build()
        .map_err(|e| dl_err("building http client", e))?;

    let mut req = client.get(url).bearer_auth(bearer_token);
    if existing > 0 {
        req = req.header("range", format!("bytes={existing}-"));
    }

    let mut resp = req.send().await.map_err(|e| dl_err("request", e))?;
    let mut status = resp.status();

    // A ranged resume can come back 416 (Range Not Satisfiable) when the local
    // partial is already at — or beyond — the object's size: the file finished
    // on an earlier run. If it's exactly complete we're done; if it's somehow
    // larger than the source it's corrupt, so discard it and re-fetch fresh.
    // Without this, an interrupted multi-file download could never be retried —
    // the already-complete files would 416 forever.
    if existing > 0 && status.as_u16() == 416 {
        let total = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit('/').next())
            .and_then(|n| n.trim().parse::<u64>().ok());
        if total == Some(existing) {
            on_progress(existing, total);
            return Ok(DownloadReport {
                url: url.to_string(),
                dest: dest.to_path_buf(),
                bytes: existing,
                resumed: true,
            });
        }
        let _ = std::fs::remove_file(dest);
        existing = 0;
        resp = client
            .get(url)
            .bearer_auth(bearer_token)
            .send()
            .await
            .map_err(|e| dl_err("request", e))?;
        status = resp.status();
    }

    if !status.is_success() {
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(none)")
            .to_string();
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Download(format!(
            "{url} returned {} (location: {location}): {}",
            status.as_u16(),
            body.chars().take(160).collect::<String>()
        )));
    }

    // 206 Partial Content => our Range was honored and we append; anything else
    // (e.g. 200) means a fresh body, so start the file over.
    let resumed = status.as_u16() == 206 && existing > 0;
    let content_len = resp.content_length();
    let total = match (resumed, content_len) {
        (true, Some(remaining)) => Some(existing + remaining),
        (false, Some(len)) => Some(len),
        _ => None,
    };

    let mut file = if resumed {
        std::fs::OpenOptions::new().append(true).open(dest)?
    } else {
        std::fs::File::create(dest)?
    };

    let mut downloaded = if resumed { existing } else { 0 };
    on_progress(downloaded, total);

    while let Some(chunk) = resp.chunk().await.map_err(|e| dl_err("reading body", e))? {
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    file.flush()?;

    Ok(DownloadReport {
        url: url.to_string(),
        dest: dest.to_path_buf(),
        bytes: downloaded,
        resumed,
    })
}
