//! minihoard MCP server (stdio, JSON-RPC 2.0).
//!
//! Exposes the minihoard library over MCP so an assistant can browse and
//! download by chat. Hand-rolled protocol (newline-delimited JSON-RPC) to avoid
//! SDK churn. Logs go to stderr only — stdout is the protocol channel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Progress state for one background download job. Long downloads run detached
/// so the assistant can poll `job_status` and narrate, instead of the MCP call
/// blocking silently for minutes.
#[derive(Default, Clone)]
struct Job {
    total: usize,
    done: usize,
    object: String,
    current: String,
    phase: String,
    finished: bool,
    summary: String,
    error: Option<String>,
}

static JOBS: LazyLock<Mutex<HashMap<u64, Job>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static JOB_SEQ: AtomicU64 = AtomicU64::new(1);

fn update_job(id: u64, f: impl FnOnce(&mut Job)) {
    if let Ok(mut m) = JOBS.lock() {
        if let Some(j) = m.get_mut(&id) {
            f(j);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[mcp] bad json: {e}");
                continue;
            }
        };

        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").cloned();

        // Notifications have no id → no response.
        if id.is_none() {
            eprintln!("[mcp] notification: {method}");
            continue;
        }
        let id = id.unwrap();

        let response = match method {
            "initialize" => {
                let pv = req["params"]["protocolVersion"]
                    .as_str()
                    .unwrap_or("2024-11-05")
                    .to_string();
                ok(id, json!({
                    "protocolVersion": pv,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "minihoard", "version": "0.1.0" }
                }))
            }
            "tools/list" => ok(id, json!({ "tools": tool_defs() })),
            "tools/call" => {
                let name = req["params"]["name"].as_str().unwrap_or("").to_string();
                let args = req["params"]["arguments"].clone();
                match call_tool(&name, args).await {
                    Ok(text) => ok(id, json!({ "content": [{ "type": "text", "text": text }] })),
                    Err(e) => ok(
                        id,
                        json!({ "content": [{ "type": "text", "text": format!("Error: {e}") }], "isError": true }),
                    ),
                }
            }
            "ping" => ok(id, json!({})),
            other => err(id, -32601, &format!("method not found: {other}")),
        };

        let mut buf = serde_json::to_vec(&response)?;
        buf.push(b'\n');
        stdout.write_all(&buf).await?;
        stdout.flush().await?;
    }
    Ok(())
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_defs() -> Value {
    json!([
        {
            "name": "status",
            "description": "Check minihoard auth: the OAuth account (whoami) and whether the website session cookie used for library listing is still valid.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_library",
            "description": "List objects in the user's MyMiniFactory library. Filter by release month (YYYY-MM), creator name, text, or source channel (TRIBE/PURCHASE/kickstarter/etc.). Marks which are already downloaded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "month": { "type": "string", "description": "Release month, e.g. 2026-06" },
                    "creator": { "type": "string", "description": "Creator name substring (case-insensitive)" },
                    "search": { "type": "string", "description": "Object name/tag substring (case-insensitive)" },
                    "source": { "type": "string", "description": "Source channel substring, e.g. tribe / purchase / kickstarter" },
                    "undownloaded": { "type": "boolean", "description": "Only items not yet downloaded" },
                    "limit": { "type": "integer", "description": "Max items to return (default 50)" }
                }
            }
        },
        {
            "name": "preview_download",
            "description": "Preview what downloading the given object ids would fetch (filenames and sizes), WITHOUT downloading. Use this to confirm with the user before download_objects.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" }, "description": "Object ids" }
                },
                "required": ["ids"]
            }
        },
        {
            "name": "download_objects",
            "description": "Start downloading the given object ids (all their files), auto-unpack zips, clean, and reorganize. Runs in the BACKGROUND and returns immediately with a job id — it does NOT block until done. Confirm with the user first using preview_download, then poll job_status with the returned id to report progress.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" }, "description": "Object ids" }
                },
                "required": ["ids"]
            }
        },
        {
            "name": "job_status",
            "description": "Check progress of a background job started by download_objects, pack, or unpack. Pass the job id for that job's status (done/total, current item, phase), or omit it to list all jobs. Poll this every few seconds during long work and relay progress to the user.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job": { "type": "integer", "description": "Job id (omit to list all jobs)" }
                }
            }
        },
        {
            "name": "config",
            "description": "Show the current configuration and where files are stored — the unpack/downloads location, config file, manifest, and data dir. Use when the user asks where their downloads or releases are.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "pack",
            "description": "Repack release folder(s) into archives for backup. Runs in the BACKGROUND and returns a job id immediately — poll job_status. tar.zst is best compression and supports --split into fixed-size volumes (e.g. 4G chunks); zip is broadly supported and can also be --split (a byte-split of the finished archive, rejoined to restore). Each archive gets a .json sidecar index.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Folders to pack (each becomes its own archive)" },
                    "format": { "type": "string", "enum": ["tarzst", "zip"], "description": "Archive format (default tarzst)" },
                    "level": { "type": "integer", "description": "zstd level 1-22 (default 19; tar.zst only)" },
                    "split": { "type": "string", "description": "Volume size for chunked backup (tar.zst or zip). Uses SI decimal units: '4G' = 4,000,000,000 bytes (fits Telegram 4 GB limit), '2G' = 2,000,000,000 bytes (fits Telegram 2 GB non-premium limit). Accepts K/M/G/T (decimal) or KiB/MiB/GiB/TiB (binary). Raw byte counts also accepted, e.g. '3800000000'. Do NOT use GiB/MiB if targeting upload limits — those are larger than their GB/MB equivalents." },
                    "out": { "type": "string", "description": "Output directory (default: alongside each source folder)" },
                    "name": { "type": "string", "description": "Archive base filename, verbatim (e.g. 'Dungeon Classics - 2026-04'). Single folder only; defaults to the folder name." }
                },
                "required": ["paths"]
            }
        },
        {
            "name": "unpack",
            "description": "Extract a .zip or .tar.zst archive (point at the .001 volume for a split set) into the unpack directory. Runs in the BACKGROUND and returns a job id — poll job_status. Optionally delete the archive (all volumes + sidecar) after a successful extraction.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "archive": { "type": "string", "description": "Path to a .zip or .tar.zst archive" },
                    "delete_archive": { "type": "boolean", "description": "Delete the archive after extracting (default false)" }
                },
                "required": ["archive"]
            }
        },
        {
            "name": "tidy",
            "description": "Tidy existing release folders: strip macOS junk and collapse redundant single-folder nesting (Release/Release/files → Release/files). With no paths, tidies the whole library. Runs in the BACKGROUND — poll job_status. Never merges folders that have 2+ real children.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Folders to tidy (omit to tidy every release in the library)" }
                }
            }
        }
    ])
}

async fn call_tool(name: &str, args: Value) -> Result<String> {
    match name {
        "status" => status().await,
        "list_library" => list_library(args).await,
        "preview_download" => preview_download(ids_arg(&args)?).await,
        "download_objects" => download_objects(ids_arg(&args)?).await,
        "job_status" => job_status(args).await,
        "config" => config_info().await,
        "pack" => pack(args).await,
        "unpack" => unpack(args).await,
        "tidy" => tidy(args).await,
        other => anyhow::bail!("unknown tool: {other}"),
    }
}

fn ids_arg(args: &Value) -> Result<Vec<u64>> {
    let ids = args["ids"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("`ids` must be an array of object ids"))?
        .iter()
        .filter_map(|v| v.as_u64())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        anyhow::bail!("no valid object ids given");
    }
    Ok(ids)
}


async fn status() -> Result<String> {
    let config = mmf_core::Config::load()?;
    let mut out = String::new();

    match mmf_core::auth::access_token(&config.client_id).await {
        Ok(token) => {
            let client = mmf_core::api::Client::with_bearer(token);
            match client.current_user().await {
                Ok(u) => out.push_str(&format!(
                    "OAuth: ok — logged in as {} (id {}).\n",
                    u["username"].as_str().unwrap_or("?"),
                    u["id"].as_u64().unwrap_or(0)
                )),
                Err(e) => out.push_str(&format!("OAuth: token ok but /user failed: {e}\n")),
            }
        }
        Err(e) => out.push_str(&format!("OAuth: not usable — {e}\n")),
    }

    match mmf_core::auth::TokenStore::session_cookie()? {
        Some(cookie) => match mmf_core::library::fetch_library(&cookie).await {
            Ok(list) => out.push_str(&format!(
                "Library cookie: ok — {} entries readable.",
                list.len()
            )),
            Err(e) => out.push_str(&format!("Library cookie: NOT working — {e}")),
        },
        None => out.push_str("Library cookie: not set (run `minihoard set-cookie`)."),
    }
    Ok(out)
}

async fn load_library() -> Result<Vec<mmf_core::library::LibraryEntry>> {
    let cookie = mmf_core::auth::TokenStore::session_cookie()?
        .ok_or_else(|| anyhow::anyhow!("no session cookie — run `minihoard set-cookie`"))?;
    Ok(mmf_core::library::dedupe(
        mmf_core::library::fetch_library(&cookie).await?,
    ))
}

async fn list_library(args: Value) -> Result<String> {
    let mut entries = load_library().await?;
    let manifest = mmf_core::manifest::Manifest::load(&mmf_core::Config::default_data_dir()?)?;

    let month = args["month"].as_str().map(|m| m.replace('-', ""));
    let creator = args["creator"].as_str().map(|c| c.to_lowercase());
    let search = args["search"].as_str().map(|s| s.to_lowercase());
    let source = args["source"].as_str().map(|s| s.to_lowercase());
    let undownloaded = args["undownloaded"].as_bool().unwrap_or(false);
    let limit = args["limit"].as_u64().unwrap_or(50) as usize;

    entries.retain(|e| {
        if let Some(m) = &month {
            if e.yearmonth().as_deref() != Some(m.as_str()) {
                return false;
            }
        }
        if let Some(c) = &creator {
            if !e.creator_name.as_deref().map(|n| n.to_lowercase().contains(c)).unwrap_or(false) {
                return false;
            }
        }
        if let Some(s) = &search {
            if !e.name.to_lowercase().contains(s)
                && !e.tags.iter().any(|t| t.to_lowercase().contains(s))
            {
                return false;
            }
        }
        if let Some(src) = &source {
            if !e.source.as_deref().map(|s| s.to_lowercase().contains(src)).unwrap_or(false) {
                return false;
            }
        }
        if undownloaded && manifest.contains(e.original_id) {
            return false;
        }
        true
    });
    entries.sort_by(|a, b| b.library_added_at.cmp(&a.library_added_at));

    let total = entries.len();
    let mut out = format!("{total} match(es). Showing up to {limit}:\n");
    for e in entries.iter().take(limit) {
        let mark = if manifest.contains(e.original_id) { "[downloaded]" } else { "[ ]" };
        let added = e.library_added_at.as_deref().and_then(|s| s.split('T').next()).unwrap_or("?");
        out.push_str(&format!(
            "{mark} id={} {:?} — {} ({}) added {added}\n",
            e.original_id,
            e.name,
            e.creator_name.as_deref().unwrap_or("?"),
            e.month_label().unwrap_or_else(|| "—".into()),
        ));
    }
    Ok(out)
}

/// Fetch an object's file list via the OAuth API.
async fn object_files(client: &mmf_core::api::Client, id: u64) -> Result<(String, Vec<Value>)> {
    let object = client.get(&format!("/objects/{id}")).await?;
    let name = object["name"].as_str().unwrap_or("object").to_string();
    let files = object["files"]["items"].as_array().cloned().unwrap_or_default();
    Ok((name, files))
}

async fn preview_download(ids: Vec<u64>) -> Result<String> {
    let config = mmf_core::Config::load()?;
    let token = mmf_core::auth::access_token(&config.client_id).await?;
    let client = mmf_core::api::Client::with_bearer(token);

    let mut out = String::new();
    let mut grand: u64 = 0;
    for id in ids {
        let (name, files) = object_files(&client, id).await?;
        let mut obj_bytes: u64 = 0;
        let mut lines = String::new();
        for f in &files {
            let fname = f["filename"].as_str().unwrap_or("?");
            let size: u64 = f["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
            obj_bytes += size;
            lines.push_str(&format!("    {fname} ({} MB)\n", size / 1_048_576));
        }
        grand += obj_bytes;
        out.push_str(&format!("id={id} {name} — {} MB\n{lines}", obj_bytes / 1_048_576));
    }
    out.push_str(&format!("\nTotal: {} MB across the selection.", grand / 1_048_576));
    Ok(out)
}

/// Kick off a background download and return a job id immediately. Auth/config
/// are validated up-front so obvious failures surface synchronously; the actual
/// transfer + unpack runs detached and is observed via `job_status`.
async fn download_objects(ids: Vec<u64>) -> Result<String> {
    let config = mmf_core::Config::load()?;
    let token = mmf_core::auth::access_token(&config.client_id).await?;
    let cookie = mmf_core::auth::TokenStore::session_cookie()?;

    let job_id = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
    JOBS.lock().unwrap().insert(
        job_id,
        Job {
            total: ids.len(),
            phase: "starting".into(),
            current: "preparing…".into(),
            ..Default::default()
        },
    );

    let count = ids.len();
    tokio::spawn(async move {
        use mmf_core::pipeline::Progress;
        use std::collections::HashMap;

        let opts = mmf_core::pipeline::Options {
            keep_archive: false,
            concurrency: config.download_concurrency as usize,
        };
        let mut inflight: HashMap<String, u64> = HashMap::new();
        let mut done_count = 0usize;

        let result = mmf_core::pipeline::download_objects(
            &config,
            &token,
            cookie.as_deref(),
            &ids,
            &opts,
            |p| match p {
                Progress::ObjectStart { total, name, .. } => update_job(job_id, |j| {
                    j.total = total;
                    j.phase = "downloading".into();
                    inflight.insert(name.clone(), 0);
                    j.current = describe_inflight(&inflight);
                }),
                Progress::File { object, done, .. } => {
                    inflight.insert(object, done);
                    update_job(job_id, |j| j.current = describe_inflight(&inflight));
                }
                Progress::ObjectDone { name, bytes, files, .. } => {
                    inflight.remove(&name);
                    done_count += 1;
                    update_job(job_id, |j| {
                        j.done = done_count;
                        j.current = format!(
                            "finished {name} ({} MB, {files} files); {}",
                            bytes / 1_048_576,
                            describe_inflight(&inflight)
                        );
                    });
                }
                Progress::ObjectFailed { name, .. } => {
                    inflight.remove(&name);
                }
            },
        )
        .await;

        match result {
            Ok(outcomes) => update_job(job_id, |j| {
                j.finished = true;
                j.done = j.total;
                j.phase = "done".into();
                j.current = "done".into();
                if outcomes.is_empty() {
                    j.summary =
                        "Nothing downloaded — none of the ids had files (do you own them?)."
                            .into();
                } else {
                    let mut s = format!("Downloaded {} object(s):", outcomes.len());
                    for o in &outcomes {
                        s.push_str(&format!(
                            "\n✓ {} ({} MB, {} files) → {}",
                            o.name,
                            o.bytes / 1_048_576,
                            o.file_count,
                            o.dir.display()
                        ));
                    }
                    j.summary = s;
                }
            }),
            Err(e) => update_job(job_id, |j| {
                j.finished = true;
                j.phase = "error".into();
                j.current = "error".into();
                j.error = Some(e.to_string());
            }),
        }
    });

    Ok(format!(
        "Started background job #{job_id} for {count} object(s). It runs in the background — \
         call job_status with job={job_id} every few seconds to follow progress and report it \
         to the user."
    ))
}

async fn job_status(args: Value) -> Result<String> {
    let map = JOBS
        .lock()
        .map_err(|_| anyhow::anyhow!("job registry unavailable"))?;
    if let Some(id) = args["job"].as_u64() {
        let job = map
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("no job #{id}"))?;
        return Ok(render_job(id, job));
    }
    if map.is_empty() {
        return Ok("No download jobs yet.".into());
    }
    let mut ids: Vec<u64> = map.keys().copied().collect();
    ids.sort_unstable();
    let mut out = String::new();
    for id in ids {
        out.push_str(&render_job(id, &map[&id]));
        out.push('\n');
    }
    Ok(out)
}

fn render_job(id: u64, j: &Job) -> String {
    if let Some(e) = &j.error {
        return format!("Job #{id}: ERROR after {}/{} — {e}", j.done, j.total);
    }
    if j.finished {
        return format!("Job #{id}: ✅ done ({}/{}).\n{}", j.done, j.total, j.summary);
    }
    format!(
        "Job #{id}: {} — {}/{} done. Current: {}",
        j.phase, j.done, j.total, j.current
    )
}

/// Summarize the objects currently downloading, for a job's `current` line.
fn describe_inflight(inflight: &std::collections::HashMap<String, u64>) -> String {
    if inflight.is_empty() {
        return "unpacking / finishing…".into();
    }
    let mut names: Vec<&str> = inflight.keys().map(|s| s.as_str()).collect();
    names.sort_unstable();
    format!("downloading {}: {}", names.len(), names.join(", "))
}

async fn config_info() -> Result<String> {
    match mmf_core::Config::load() {
        Ok(cfg) => Ok(cfg.describe()),
        Err(mmf_core::Error::ConfigMissing(p)) => Ok(format!(
            "No config yet (expected at {}). Run `minihoard configure` in a terminal.",
            p.display()
        )),
        Err(e) => Err(e.into()),
    }
}

async fn pack(args: Value) -> Result<String> {
    use mmf_core::pack::{parse_size, PackFormat, PackOptions};

    let paths: Vec<std::path::PathBuf> = args["paths"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("`paths` must be an array of folder paths"))?
        .iter()
        .filter_map(|v| v.as_str().map(std::path::PathBuf::from))
        .collect();
    if paths.is_empty() {
        anyhow::bail!("no folders given to pack");
    }
    let name_override = args["name"].as_str().map(String::from);
    if name_override.is_some() && paths.len() > 1 {
        anyhow::bail!("`name` only works with a single folder (you gave {})", paths.len());
    }
    let format = PackFormat::parse(args["format"].as_str().unwrap_or("tarzst"))?;
    let level = args["level"].as_i64().unwrap_or(19) as i32;
    let split_bytes = match args["split"].as_str() {
        Some(s) => Some(parse_size(s)?),
        None => None,
    };
    let out = args["out"].as_str().map(std::path::PathBuf::from);

    let job_id = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
    JOBS.lock().unwrap().insert(
        job_id,
        Job {
            total: paths.len(),
            phase: "packing".into(),
            current: "starting…".into(),
            ..Default::default()
        },
    );

    let count = paths.len();
    tokio::task::spawn_blocking(move || {
        let mut summary = String::new();
        for (i, src) in paths.iter().enumerate() {
            // Archive base name: the override (single-folder only) or folder name.
            let name = name_override.clone().unwrap_or_else(|| {
                src.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("archive")
                    .to_string()
            });
            update_job(job_id, |j| {
                j.done = i;
                j.object = name.clone();
                j.current = format!("packing {name}");
            });
            let out_dir = out.clone().unwrap_or_else(|| {
                src.parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf()
            });
            let opts = PackOptions {
                format,
                level,
                split_bytes,
                write_sidecar: true,
            };
            let nm = name.clone();
            match mmf_core::pack::pack_dir(src, &out_dir, &opts, name_override.as_deref(), |done| {
                update_job(job_id, |j| {
                    j.current = format!("packing {nm} — {} MB written", done / 1_048_576);
                });
            }) {
                Ok(report) => summary.push_str(&format!(
                    "\n✓ {name} ({} files, {} MB → {} MB{}) → {}",
                    report.file_count,
                    report.input_bytes / 1_048_576,
                    report.output_bytes / 1_048_576,
                    if report.outputs.len() > 1 {
                        format!(", {} volumes", report.outputs.len())
                    } else {
                        String::new()
                    },
                    report.outputs[0].display(),
                )),
                Err(e) => {
                    update_job(job_id, |j| {
                        j.finished = true;
                        j.phase = "error".into();
                        j.error = Some(format!("packing {name}: {e}"));
                    });
                    return;
                }
            }
        }
        update_job(job_id, |j| {
            j.finished = true;
            j.done = j.total;
            j.phase = "done".into();
            j.current = "done".into();
            j.summary = format!("Packed {} folder(s):{summary}", j.total);
        });
    });

    Ok(format!(
        "Started background pack job #{job_id} for {count} folder(s). Poll job_status with job={job_id}."
    ))
}

async fn unpack(args: Value) -> Result<String> {
    let archive = args["archive"]
        .as_str()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("`archive` (path) is required"))?;
    let delete = args["delete_archive"].as_bool().unwrap_or(false);
    let config = mmf_core::Config::load()?;
    let dest = config.unpack_dir.clone();

    let job_id = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
    JOBS.lock().unwrap().insert(
        job_id,
        Job {
            total: 1,
            phase: "unpacking".into(),
            current: format!(
                "extracting {}",
                archive.file_name().and_then(|s| s.to_str()).unwrap_or("archive")
            ),
            ..Default::default()
        },
    );

    tokio::task::spawn_blocking(move || {
        let result = if mmf_core::pack::is_tar_zst(&archive) {
            mmf_core::pack::unpack_tar_zst(&archive, &dest)
        } else {
            mmf_core::unpack::unpack_zip(&archive, &dest).map(|r| {
                mmf_core::clean::strip_apple_artifacts(&r.dest);
                let _ = mmf_core::clean::flatten_single_dir(&r.dest);
                r.files_written
            })
        };
        match result {
            Ok(n) => {
                let mut note = String::new();
                if delete {
                    match mmf_core::pack::remove_archive_files(&archive) {
                        Ok(removed) => note = format!(" Removed {removed} archive file(s).",),
                        Err(e) => note = format!(" (could not delete archive: {e})"),
                    }
                }
                update_job(job_id, |j| {
                    j.finished = true;
                    j.done = 1;
                    j.phase = "done".into();
                    j.current = "done".into();
                    j.summary = format!("Unpacked {n} files to {}.{note}", dest.display());
                });
            }
            Err(e) => update_job(job_id, |j| {
                j.finished = true;
                j.phase = "error".into();
                j.error = Some(e.to_string());
            }),
        }
    });

    Ok(format!(
        "Started background unpack job #{job_id}. Poll job_status with job={job_id}."
    ))
}

async fn tidy(args: Value) -> Result<String> {
    let targets: Vec<std::path::PathBuf> = match args["paths"].as_array() {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(std::path::PathBuf::from))
            .collect(),
        None => {
            let config = mmf_core::Config::load()?;
            mmf_core::clean::library_release_dirs(&config.unpack_dir)
        }
    };
    if targets.is_empty() {
        return Ok("Nothing to tidy.".into());
    }

    let job_id = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
    JOBS.lock().unwrap().insert(
        job_id,
        Job {
            total: targets.len(),
            phase: "tidying".into(),
            current: "starting…".into(),
            ..Default::default()
        },
    );

    let count = targets.len();
    tokio::task::spawn_blocking(move || {
        let mut renamed = 0usize;
        let mut summary = String::new();
        for (i, dir) in targets.iter().enumerate() {
            let label = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("folder")
                .to_string();
            update_job(job_id, |j| {
                j.done = i;
                j.current = format!("tidying {label}");
            });
            if !dir.is_dir() {
                continue;
            }
            match mmf_core::clean::tidy_dir(dir) {
                Ok(final_dir) if final_dir != *dir => {
                    renamed += 1;
                    summary.push_str(&format!("\n✓ {} → {}", dir.display(), final_dir.display()));
                }
                Ok(_) => {}
                Err(e) => summary.push_str(&format!("\n⚠ {}: {e}", dir.display())),
            }
        }
        update_job(job_id, |j| {
            j.finished = true;
            j.done = j.total;
            j.phase = "done".into();
            j.current = "done".into();
            j.summary = format!("Tidied {} folder(s); {renamed} collapsed.{summary}", j.total);
        });
    });

    Ok(format!(
        "Started background tidy job #{job_id} for {count} folder(s). Poll job_status with job={job_id}."
    ))
}
