//! minihoard MCP server (stdio, JSON-RPC 2.0).
//!
//! Exposes the minihoard library over MCP so an assistant can browse and
//! download by chat. Hand-rolled protocol (newline-delimited JSON-RPC) to avoid
//! SDK churn. Logs go to stderr only — stdout is the protocol channel.

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
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
            "description": "List objects in the user's MyMiniFactory library. Filter by release month (YYYY-MM), creator name, or text. Marks which are already downloaded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "month": { "type": "string", "description": "Release month, e.g. 2026-06" },
                    "creator": { "type": "string", "description": "Creator name substring (case-insensitive)" },
                    "search": { "type": "string", "description": "Object name/tag substring (case-insensitive)" },
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
            "description": "Download the given object ids (all their files), auto-unpack zips, and record them. Confirm with the user first using preview_download.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" }, "description": "Object ids" }
                },
                "required": ["ids"]
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

async fn download_objects(ids: Vec<u64>) -> Result<String> {
    let config = mmf_core::Config::load()?;
    let token = mmf_core::auth::access_token(&config.client_id).await?;
    let cookie = mmf_core::auth::TokenStore::session_cookie()?;

    let outcomes = mmf_core::pipeline::download_objects(
        &config,
        &token,
        cookie.as_deref(),
        &ids,
        &mmf_core::pipeline::Options::default(),
        |_, _, _| {},
    )
    .await?;

    if outcomes.is_empty() {
        return Ok("Nothing downloaded — none of the given ids had files (do you own them?).".into());
    }
    let mut out = String::new();
    for o in &outcomes {
        out.push_str(&format!(
            "✓ {} ({} MB, {} files) → {}\n",
            o.name,
            o.bytes / 1_048_576,
            o.file_count,
            o.dir.display()
        ));
    }
    out.push_str(&format!("\nClean releases under {}", config.unpack_dir.display()));
    Ok(out)
}
