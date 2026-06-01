//! End-to-end "get me clean releases" flow: download an object's files into a
//! tidy `{creator}-{MM-YYYY}/{release}/` folder, unpack archives in place,
//! strip macOS artifacts, and (by default) delete the archive. Shared by the
//! CLI and the MCP server so both produce identical output.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::clean::{clean_name, strip_apple_artifacts};
use crate::config::Config;
use crate::error::Result;
use crate::library::LibraryEntry;

#[derive(Default)]
pub struct Options {
    /// Keep the original `.zip` after unpacking (default: delete it).
    pub keep_archive: bool,
}

/// What happened for one object.
pub struct Outcome {
    pub id: u64,
    pub name: String,
    pub dir: PathBuf,
    pub bytes: u64,
    pub file_count: usize,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A `MM-YYYY` folder label from the library entry (release month, else the
/// month it was added), or `undated`.
fn month_label(entry: Option<&LibraryEntry>) -> String {
    if let Some(e) = entry {
        if let Some(m) = e.month_label() {
            return m;
        }
        if let Some(added) = &e.library_added_at {
            if added.len() >= 7 {
                return format!("{}-{}", &added[5..7], &added[0..4]);
            }
        }
    }
    "undated".to_string()
}

/// Download + organize the given object ids. `cookie` (optional) is used only to
/// fetch library metadata for nicer foldering. `on_file(filename, done, total)`
/// reports per-file progress.
pub async fn download_objects(
    config: &Config,
    token: &str,
    cookie: Option<&str>,
    ids: &[u64],
    opts: &Options,
    mut on_file: impl FnMut(&str, u64, Option<u64>),
) -> Result<Vec<Outcome>> {
    // Library metadata (creator + month) for nicer folders; optional.
    let meta: HashMap<u64, LibraryEntry> = match cookie {
        Some(c) => match crate::library::fetch_library(c).await {
            Ok(list) => crate::library::dedupe(list)
                .into_iter()
                .map(|e| (e.original_id, e))
                .collect(),
            Err(_) => HashMap::new(),
        },
        None => HashMap::new(),
    };

    let client = crate::api::Client::with_bearer(token.to_string());
    let data_dir = Config::default_data_dir()?;
    let mut manifest = crate::manifest::Manifest::load(&data_dir)?;
    let mut outcomes = Vec::new();

    for &id in ids {
        let object = client.get(&format!("/objects/{id}")).await?;
        let entry = meta.get(&id);

        let name = entry
            .map(|e| e.name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| object["name"].as_str().map(String::from))
            .unwrap_or_else(|| format!("object-{id}"));
        let creator = entry
            .and_then(|e| e.creator_name.clone())
            .or_else(|| object["designer"]["name"].as_str().map(String::from))
            .unwrap_or_else(|| "unknown".to_string());
        let group = format!("{}-{}", clean_name(&creator), month_label(entry));
        let target = config.unpack_dir.join(group).join(clean_name(&name));

        let files = object["files"]["items"].as_array().cloned().unwrap_or_default();
        if files.is_empty() {
            continue; // not owned / nothing to fetch
        }
        std::fs::create_dir_all(&target)?;

        let mut total_bytes = 0u64;
        let mut file_count = 0usize;
        let mut written = Vec::new();

        for f in &files {
            let Some(url) = f["download_url"].as_str() else { continue };
            let filename = f["filename"].as_str().unwrap_or("download.bin").to_string();
            let dest = target.join(&filename);

            let fname = filename.clone();
            let cb = &mut on_file;
            let report =
                crate::download::download_to(url, &dest, token, |d, t| cb(&fname, d, t)).await?;
            total_bytes += report.bytes;

            if crate::unpack::is_archive(&dest) {
                let r = crate::unpack::unpack_zip_into(&dest, &target)?;
                file_count += r.files_written;
                if !opts.keep_archive {
                    let _ = std::fs::remove_file(&dest);
                }
            } else {
                file_count += 1;
            }
            written.push(filename);
        }

        let removed = strip_apple_artifacts(&target);
        if removed > 0 {
            // recount not needed; just informational via outcome.file_count
        }

        manifest.record(id, &name, written, now_unix());
        manifest.save(&data_dir)?;

        outcomes.push(Outcome {
            id,
            name,
            dir: target,
            bytes: total_bytes,
            file_count,
        });
    }

    Ok(outcomes)
}
