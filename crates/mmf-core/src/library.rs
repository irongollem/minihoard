//! Library auto-discovery via the website's internal data-library API.
//!
//! `GET /api/data-library/objectPreviews` returns the user's entire library as
//! a flat JSON array — every owned object with enough metadata to group by
//! release month/creator and to sync incrementally. This endpoint lives on the
//! Cloudflare-fronted website host, so we reach it with the same Chrome-
//! impersonating client used for downloads.

use serde::Deserialize;
use wreq_util::Emulation;

use crate::error::{Error, Result};

const OBJECT_PREVIEWS_URL: &str =
    "https://www.myminifactory.com/api/data-library/objectPreviews";

/// One entry in the user's library. Unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct LibraryEntry {
    /// The object id used everywhere else (e.g. for `api/v2/objects/{id}`).
    #[serde(rename = "originalId")]
    pub original_id: u64,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    /// How it entered the library: "TRIBE", "PURCHASE", etc.
    #[serde(default)]
    pub source: Option<String>,
    /// Encodes creator + month, e.g.
    /// `type:tribes-tier;owner:1448840;tribe:772;tier:1845;yearmonth:202606`.
    #[serde(default)]
    pub release: Option<String>,
    #[serde(rename = "creatorName", default)]
    pub creator_name: Option<String>,
    /// When it was added to the library (ISO-8601) — used for incremental sync.
    #[serde(rename = "libraryAddedAt", default)]
    pub library_added_at: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub tags: Vec<String>,
}

/// Deserialize a field that may be present-but-null into its `Default`.
fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

impl LibraryEntry {
    /// The raw `yearmonth` from `release`, e.g. `"202606"`.
    pub fn yearmonth(&self) -> Option<String> {
        let release = self.release.as_deref()?;
        release.split(';').find_map(|kv| {
            kv.strip_prefix("yearmonth:").map(|s| s.to_string())
        })
    }

    /// A `MM-YYYY` label derived from `yearmonth` (e.g. `"06-2026"`), or none.
    pub fn month_label(&self) -> Option<String> {
        let ym = self.yearmonth()?;
        if ym.len() == 6 {
            Some(format!("{}-{}", &ym[4..6], &ym[0..4]))
        } else {
            None
        }
    }
}

/// Fetch the full library listing using the website session cookie (the
/// endpoint ignores OAuth). `cookie` is a raw `Cookie:` header copied from a
/// logged-in browser.
pub async fn fetch_library(cookie: &str) -> Result<Vec<LibraryEntry>> {
    let client = wreq::Client::builder()
        .emulation(Emulation::Chrome137)
        .redirect(wreq::redirect::Policy::limited(10))
        .build()
        .map_err(|e| Error::Api {
            status: 0,
            message: format!("building http client: {e}"),
        })?;

    let resp = client
        .get(OBJECT_PREVIEWS_URL)
        .header("accept", "application/json")
        .header("cookie", cookie)
        .send()
        .await
        .map_err(|e| Error::Api {
            status: 0,
            message: format!("requesting objectPreviews: {e}"),
        })?;

    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();

    // Not logged in / expired cookie => the site serves an HTML page instead.
    if body.trim_start().starts_with('<') || !(200..300).contains(&status) {
        return Err(Error::Auth(
            "library listing returned a web page, not data — your session cookie is \
             missing or expired. Re-run `minihoard set-cookie` with a fresh Cookie \
             header from a logged-in browser."
                .into(),
        ));
    }

    serde_json::from_str(&body).map_err(|e| Error::Api {
        status,
        message: format!(
            "parsing objectPreviews ({e}); body starts: {:?}",
            body.chars().take(120).collect::<String>()
        ),
    })
}

/// De-duplicate entries by `original_id`, keeping the first occurrence.
pub fn dedupe(entries: Vec<LibraryEntry>) -> Vec<LibraryEntry> {
    let mut seen = std::collections::HashSet::new();
    entries
        .into_iter()
        .filter(|e| seen.insert(e.original_id))
        .collect()
}
