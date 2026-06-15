//! End-to-end "get me clean releases" flow: download an object's files into a
//! tidy `{creator}-{MM-YYYY}/{release}/` folder, unpack archives in place,
//! strip macOS artifacts, and (by default) delete the archive. Shared by the
//! CLI and the MCP server so both produce identical output.
//!
//! Objects are processed with bounded concurrency (see [`Options::concurrency`]):
//! several download in parallel, each still handling its own files sequentially.
//! Progress is reported as a stream of [`Progress`] events from a single driver
//! task, so callers don't need thread-safe callbacks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_util::stream::StreamExt;

use crate::clean::{clean_name, flatten_single_dir, strip_apple_artifacts};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::library::LibraryEntry;

pub struct Options {
    /// Keep the original `.zip` after unpacking (default: delete it).
    pub keep_archive: bool,
    /// How many objects to download in parallel (clamped to ≥1).
    pub concurrency: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            keep_archive: false,
            concurrency: 5,
        }
    }
}

/// What happened for one object.
pub struct Outcome {
    pub id: u64,
    pub name: String,
    pub dir: PathBuf,
    pub bytes: u64,
    pub file_count: usize,
}

/// Progress events emitted while downloading. Delivered to the caller's callback
/// one at a time from a single driver task (so the callback can be a plain
/// `FnMut`, no synchronization needed).
pub enum Progress {
    /// An object started downloading. `index` is 1-based over `total` objects.
    ObjectStart {
        index: usize,
        total: usize,
        name: String,
    },
    /// Byte progress for the file currently downloading within `object`.
    File {
        object: String,
        filename: String,
        done: u64,
        total: Option<u64>,
    },
    /// An object finished: downloaded + unpacked + cleaned.
    ObjectDone {
        name: String,
        bytes: u64,
        files: usize,
    },
    /// An object could not be completed (e.g. not owned, or an error).
    ObjectFailed { name: String, error: String },
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

/// Immutable state shared across the concurrent per-object tasks.
struct Shared {
    client: crate::api::Client,
    meta: HashMap<u64, LibraryEntry>,
    unpack_dir: PathBuf,
    token: String,
    keep_archive: bool,
    manifest: Mutex<crate::manifest::Manifest>,
    data_dir: PathBuf,
}

/// Download + organize the given object ids, up to `opts.concurrency` at once.
/// `cookie` (optional) is used only to fetch library metadata for nicer
/// foldering. `on` receives [`Progress`] events as work proceeds.
pub async fn download_objects(
    config: &Config,
    token: &str,
    cookie: Option<&str>,
    ids: &[u64],
    opts: &Options,
    mut on: impl FnMut(Progress),
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

    let data_dir = Config::default_data_dir()?;
    let manifest = crate::manifest::Manifest::load(&data_dir)?;
    let shared = Arc::new(Shared {
        client: crate::api::Client::with_bearer(token.to_string()),
        meta,
        unpack_dir: config.unpack_dir.clone(),
        token: token.to_string(),
        keep_archive: opts.keep_archive,
        manifest: Mutex::new(manifest),
        data_dir,
    });

    let total = ids.len();
    let concurrency = opts.concurrency.max(1);
    let ids = ids.to_vec();

    // Progress events and per-object results flow back over channels so the
    // single driver loop below owns the (non-Send) `on` callback.
    let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
    let (rtx, mut rrx) = tokio::sync::mpsc::unbounded_channel::<Result<Option<Outcome>>>();

    let worker = tokio::spawn(async move {
        let mut stream = futures_util::stream::iter(ids.into_iter().enumerate().map(|(i, id)| {
            let shared = shared.clone();
            let ptx = ptx.clone();
            async move { process_object(i + 1, total, id, shared, ptx).await }
        }))
        .buffer_unordered(concurrency);

        while let Some(res) = stream.next().await {
            let _ = rtx.send(res);
        }
        // `ptx`/`rtx` drop here, closing both channels.
    });

    let mut outcomes = Vec::new();
    let mut first_err: Option<Error> = None;
    loop {
        tokio::select! {
            Some(p) = prx.recv() => on(p),
            Some(r) = rrx.recv() => match r {
                Ok(Some(o)) => outcomes.push(o),
                Ok(None) => {}
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            },
            else => break,
        }
    }
    let _ = worker.await;

    if outcomes.is_empty() {
        if let Some(e) = first_err {
            return Err(e);
        }
    }
    // Stable order by id for predictable output.
    outcomes.sort_by_key(|o| o.id);
    Ok(outcomes)
}

/// Download one object's files, unpack, clean, and record it. Returns `Ok(None)`
/// when the object has nothing to fetch (e.g. not owned).
async fn process_object(
    index: usize,
    total: usize,
    id: u64,
    shared: Arc<Shared>,
    ptx: tokio::sync::mpsc::UnboundedSender<Progress>,
) -> Result<Option<Outcome>> {
    let object = shared.client.get(&format!("/objects/{id}")).await?;
    let entry = shared.meta.get(&id);

    let name = entry
        .map(|e| e.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| object["name"].as_str().map(String::from))
        .unwrap_or_else(|| format!("object-{id}"));
    let _ = ptx.send(Progress::ObjectStart {
        index,
        total,
        name: name.clone(),
    });

    let creator = entry
        .and_then(|e| e.creator_name.clone())
        .or_else(|| object["designer"]["name"].as_str().map(String::from))
        .unwrap_or_else(|| "unknown".to_string());
    let group = format!("{}-{}", clean_name(&creator), month_label(entry));
    let target = shared.unpack_dir.join(group).join(clean_name(&name));

    let files = object["files"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if files.is_empty() {
        let _ = ptx.send(Progress::ObjectFailed {
            name,
            error: "no downloadable files (do you own it?)".into(),
        });
        return Ok(None);
    }
    std::fs::create_dir_all(&target)?;

    let mut total_bytes = 0u64;
    let mut file_count = 0usize;
    let mut written = Vec::new();

    for f in &files {
        let Some(url) = f["download_url"].as_str() else {
            continue;
        };
        let filename = f["filename"].as_str().unwrap_or("download.bin").to_string();
        let dest = target.join(&filename);

        let report = {
            let ptx = ptx.clone();
            let object_name = name.clone();
            let fname = filename.clone();
            crate::download::download_to(url, &dest, &shared.token, move |done, total| {
                let _ = ptx.send(Progress::File {
                    object: object_name.clone(),
                    filename: fname.clone(),
                    done,
                    total,
                });
            })
            .await?
        };
        total_bytes += report.bytes;

        if crate::unpack::is_archive(&dest) {
            let dest2 = dest.clone();
            // Unpack each sub-zip into its OWN folder (`foo.zip` -> `foo/`) so
            // models stay separate. Extracting straight into the release folder
            // would dump a zip's bare `Supported/`/`Unsupported/` at the top and
            // merge them across models. Redundant `foo/foo` nesting is then
            // collapsed, but a lone `Supported/` is preserved.
            let stem = std::path::Path::new(&filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(crate::clean::clean_name)
                .unwrap_or_else(|| "release".to_string());
            let subdir = target.join(stem);
            let r = tokio::task::spawn_blocking(move || {
                let report = crate::unpack::unpack_zip_into(&dest2, &subdir)?;
                crate::clean::strip_apple_artifacts(&subdir);
                let _ = crate::clean::flatten_single_dir(&subdir);
                Ok::<_, Error>(report)
            })
            .await
            .map_err(|e| Error::Unpack(format!("unpack task: {e}")))??;
            file_count += r.files_written;
            if !shared.keep_archive {
                let _ = std::fs::remove_file(&dest);
            }
        } else {
            file_count += 1;
        }
        written.push(filename);
    }

    // Strip macOS junk and collapse redundant single-folder nesting (off-thread:
    // these are synchronous filesystem walks). May rename `target`.
    let final_dir = {
        let target2 = target.clone();
        tokio::task::spawn_blocking(move || {
            strip_apple_artifacts(&target2);
            flatten_single_dir(&target2).unwrap_or(target2)
        })
        .await
        .map_err(|e| Error::Unpack(format!("cleanup task: {e}")))?
    };

    if let Ok(mut m) = shared.manifest.lock() {
        m.record(id, &name, written, now_unix());
        let _ = m.save(&shared.data_dir);
    }

    let _ = ptx.send(Progress::ObjectDone {
        name: name.clone(),
        bytes: total_bytes,
        files: file_count,
    });

    Ok(Some(Outcome {
        id,
        name,
        dir: final_dir,
        bytes: total_bytes,
        file_count,
    }))
}
