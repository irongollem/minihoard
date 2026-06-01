use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use mmf_core::config::Config;

const REPO: &str = "irongollem/minihoard";

#[derive(Parser)]
#[command(
    name = "minihoard",
    version,
    about = "Fetch, unpack, and pack your MyMiniFactory library"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create or update the config file (interactive first-run wizard).
    Configure,
    /// Log in to MyMiniFactory via the browser (stores a refresh token).
    Login,
    /// Forget the stored credentials.
    Logout,
    /// Print the authenticated MMF account (proves login works).
    Whoami,
    /// Store the MMF website session cookie (paste the Cookie header from a
    /// logged-in browser) — needed for full-library listing.
    SetCookie,
    /// Probe authenticated endpoints to discover your library (M0 spike).
    Explore {
        /// Optional object id you OWN, to confirm files + download_url appear.
        #[arg(long)]
        object: Option<u64>,
    },
    /// List the releases available in your library (filterable, for selection).
    List {
        /// Filter by release month, e.g. 2026-06 or 202606.
        #[arg(long)]
        month: Option<String>,
        /// Filter by creator name (case-insensitive substring).
        #[arg(long)]
        creator: Option<String>,
        /// Filter by object name/tag text (case-insensitive substring).
        #[arg(long)]
        search: Option<String>,
        /// Only show items not yet downloaded.
        #[arg(long)]
        undownloaded: bool,
        /// Show at most N items (default 60).
        #[arg(long, default_value_t = 60)]
        limit: usize,
    },
    /// Download one or more releases by id (unpacks, cleans, reorganizes).
    Download {
        /// Object ids to download.
        ids: Vec<u64>,
        /// Keep the original .zip after unpacking (default: delete it).
        #[arg(long)]
        keep_archive: bool,
    },
    /// Repack release folder(s) into archives for off-site backup.
    Pack {
        /// One or more release folders to pack (each becomes its own archive).
        paths: Vec<PathBuf>,
        /// Archive format: `tarzst` (best compression, splittable) or `zip`
        /// (broadly supported, native extraction).
        #[arg(long, default_value = "tarzst")]
        format: String,
        /// zstd compression level 1-22 (tar.zst only).
        #[arg(long, default_value_t = 19)]
        level: i32,
        /// Split into fixed-size volumes, e.g. 2G or 4G (tar.zst only).
        #[arg(long)]
        split: Option<String>,
        /// Output directory (default: alongside each source folder).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Unpack a downloaded archive (.zip or .tar.zst) into the unpack directory.
    Unpack {
        /// Path to a `.zip` or `.tar.zst` archive (a `.001` volume for splits).
        archive: PathBuf,
    },
    /// Run the full monthly flow: list -> download new -> unpack.
    Sync,
    /// Update minihoard and minihoard-mcp to the latest release.
    Upgrade,
    /// Register minihoard-mcp in Claude Desktop (edits claude_desktop_config.json).
    SetupMcp,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Configure => configure(),
        Command::Login => login().await,
        Command::Logout => logout(),
        Command::Whoami => whoami().await,
        Command::SetCookie => set_cookie(),
        Command::Explore { object } => explore(object).await,
        Command::List {
            month,
            creator,
            search,
            undownloaded,
            limit,
        } => list(month, creator, search, undownloaded, limit).await,
        Command::Download { ids, keep_archive } => download(ids, keep_archive).await,
        Command::Pack {
            paths,
            format,
            level,
            split,
            out,
        } => pack(paths, format, level, split, out),
        Command::Unpack { archive } => unpack(archive),
        Command::Sync => sync().await,
        Command::Upgrade => upgrade().await,
        Command::SetupMcp => setup_mcp(),
    }
}

/// Interactive first-run wizard. Writes a non-secret config file; secrets are
/// captured later by `login`.
fn configure() -> Result<()> {
    println!("minihoard configuration\n");
    println!(
        "You need a MyMiniFactory API client id. Create one in your account\n\
         settings under the developer/API section, then paste the client id here.\n"
    );

    let existing = Config::load().ok();

    let client_id = prompt(
        "MMF API client id",
        existing.as_ref().map(|c| c.client_id.clone()),
    )?;

    // Required by MMF's token endpoint (HTTP Basic auth). Find it on your app's
    // detail page (the list of apps -> your app). Stored in the keychain only.
    println!("(Find the client secret on your MMF app's detail page.)");
    let secret = prompt_optional("MMF API client secret")?;
    if let Some(secret) = secret {
        mmf_core::auth::TokenStore::save_client_secret(&secret)?;
        println!("  stored client secret in the keychain");
    } else if mmf_core::auth::TokenStore::client_secret()?.is_none() {
        println!("  warning: no client secret stored — login will fail without it");
    }

    let data_dir = Config::default_data_dir()?;
    let download_dir = prompt_path(
        "Download directory",
        existing
            .as_ref()
            .map(|c| c.download_dir.clone())
            .unwrap_or_else(|| data_dir.join("downloads")),
    )?;
    let unpack_dir = prompt_path(
        "Unpack directory",
        existing
            .as_ref()
            .map(|c| c.unpack_dir.clone())
            .unwrap_or_else(|| data_dir.join("unpacked")),
    )?;

    let config = Config {
        client_id,
        redirect_port: existing.as_ref().map(|c| c.redirect_port).unwrap_or(8723),
        download_dir,
        unpack_dir,
        defaults: existing.map(|c| c.defaults).unwrap_or_default(),
    };
    config.save()?;

    println!("\nSaved config to {}", Config::default_path()?.display());
    println!("Next: run `minihoard login` to authenticate.");
    Ok(())
}

async fn login() -> Result<()> {
    let config = Config::load().context("load config (run `minihoard configure` first)")?;
    mmf_core::auth::login(&config.client_id, config.redirect_port)
        .await
        .context("browser login")?;
    println!("Logged in.");
    Ok(())
}

fn logout() -> Result<()> {
    mmf_core::auth::TokenStore::clear()?;
    println!("Cleared stored credentials.");
    Ok(())
}

/// Store the website session cookie (pasted from a logged-in browser).
fn set_cookie() -> Result<()> {
    println!(
        "Paste your MyMiniFactory Cookie header, then Enter.\n\
         (In the browser DevTools: Network tab → any www.myminifactory.com request →\n\
          Request Headers → copy the full `cookie:` value.)\n"
    );
    print!("cookie: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let mut cookie = input.trim();
    // Tolerate pasting the whole header line ("cookie: ...").
    if let Some(rest) = cookie.strip_prefix("cookie:").or_else(|| cookie.strip_prefix("Cookie:")) {
        cookie = rest.trim();
    }
    if cookie.is_empty() {
        anyhow::bail!("no cookie entered");
    }
    mmf_core::auth::TokenStore::save_session_cookie(cookie)?;
    println!("Saved session cookie to the keychain.");
    Ok(())
}

/// M0 proof: refresh an access token and fetch the current user.
async fn whoami() -> Result<()> {
    let config = Config::load()?;
    let token = mmf_core::auth::access_token(&config.client_id)
        .await
        .context("get access token (run `minihoard login` first)")?;
    let client = mmf_core::api::Client::with_bearer(token);
    let user = client.current_user().await.context("GET /user")?;
    println!("{}", serde_json::to_string_pretty(&user)?);
    Ok(())
}

/// M0 spike: probe authenticated endpoints and dump what we find, so we can see
/// where the library lives and whether download URLs appear with OAuth.
async fn explore(object: Option<u64>) -> Result<()> {
    let config = Config::load()?;
    let token = mmf_core::auth::access_token(&config.client_id)
        .await
        .context("get access token (run `minihoard login` first)")?;
    let client = mmf_core::api::Client::with_bearer(token);

    let user = probe(&client, "/user").await;
    let username = user
        .as_ref()
        .ok()
        .and_then(|v| v["username"].as_str().map(String::from));

    if let Some(name) = &username {
        println!("\n>>> username: {name}\n");

        // Documented authenticated listings.
        let collections = probe(&client, &format!("/users/{name}/collections")).await;
        probe(&client, &format!("/users/{name}/objects_liked")).await.ok();

        // Peek into the first collection's objects — purchases may live here.
        if let Ok(c) = &collections {
            if let Some(first) = c["items"].as_array().and_then(|a| a.first()) {
                let id = first["id"].as_u64();
                let slug = first["slug"].as_str();
                if let Some(id) = id {
                    probe(&client, &format!("/collections/{id}/objects")).await.ok();
                }
                if let Some(slug) = slug {
                    probe(&client, &format!("/users/{name}/collections/{slug}")).await.ok();
                }
            }
        }

        // Undocumented guesses for a purchases/library endpoint (404s are fine).
        for path in [
            format!("/users/{name}/objects_purchased"),
            "/user/purchases".to_string(),
            "/user/library".to_string(),
            "/user/downloads".to_string(),
        ] {
            probe(&client, &path).await.ok();
        }
    } else {
        eprintln!("could not resolve username from /user — see error above");
    }

    // The download path: object detail -> files -> a file's download_url.
    if let Some(id) = object {
        println!("\n>>> object {id}: detail, then files (look for download_url)");
        probe(&client, &format!("/objects/{id}")).await.ok();
        probe(&client, &format!("/objects/{id}/files")).await.ok();
    }
    Ok(())
}

/// GET a path and pretty-print the result (or the error) under a header.
async fn probe(
    client: &mmf_core::api::Client,
    path: &str,
) -> std::result::Result<serde_json::Value, ()> {
    println!("=== GET {path} ===");
    match client.get(path).await {
        Ok(v) => {
            // Truncate huge arrays so the spike output stays readable.
            println!("{}", preview(&v));
            Ok(v)
        }
        Err(e) => {
            println!("ERROR: {e}");
            Err(())
        }
    }
}

/// Pretty-print JSON, but cap any top-level `items`/array to the first 3 entries.
fn preview(v: &serde_json::Value) -> String {
    let mut clone = v.clone();
    if let Some(items) = clone.get_mut("items").and_then(|i| i.as_array_mut()) {
        let total = items.len();
        if total > 3 {
            items.truncate(3);
            items.push(serde_json::json!(format!("... ({total} total, truncated)")));
        }
    }
    serde_json::to_string_pretty(&clone).unwrap_or_else(|_| v.to_string())
}

async fn list(
    month: Option<String>,
    creator: Option<String>,
    search: Option<String>,
    undownloaded: bool,
    limit: usize,
) -> Result<()> {
    use std::collections::BTreeMap;

    let _config = Config::load()?;
    let cookie = mmf_core::auth::TokenStore::session_cookie()?
        .ok_or_else(|| anyhow::anyhow!("no session cookie — run `minihoard set-cookie` first"))?;

    let raw = mmf_core::library::fetch_library(&cookie)
        .await
        .context("fetch library (objectPreviews)")?;
    let mut entries = mmf_core::library::dedupe(raw);
    let manifest = mmf_core::manifest::Manifest::load(&Config::default_data_dir()?)?;

    let any_filter =
        month.is_some() || creator.is_some() || search.is_some() || undownloaded;

    // No filters → show the per-month overview to help the user choose.
    if !any_filter {
        let mut by_month: BTreeMap<String, usize> = BTreeMap::new();
        for e in &entries {
            *by_month
                .entry(e.yearmonth().unwrap_or_else(|| "unknown".into()))
                .or_default() += 1;
        }
        println!("Library: {} unique objects\n", entries.len());
        println!("By release month (use --month to filter):");
        for (ym, n) in by_month.iter().rev().take(20) {
            let label = if ym.len() == 6 {
                format!("{}-{}", &ym[4..6], &ym[0..4])
            } else {
                ym.clone()
            };
            println!("  {label:>9}  {n:>4}");
        }
        println!("\nShowing newest {limit}. Narrow with --month / --creator / --search / --undownloaded.\n");
    }

    // Apply filters.
    let month_norm = month.as_ref().map(|m| m.replace('-', ""));
    let creator_lc = creator.as_ref().map(|c| c.to_lowercase());
    let search_lc = search.as_ref().map(|s| s.to_lowercase());
    entries.retain(|e| {
        if let Some(m) = &month_norm {
            if e.yearmonth().as_deref() != Some(m.as_str()) {
                return false;
            }
        }
        if let Some(c) = &creator_lc {
            if !e
                .creator_name
                .as_deref()
                .map(|n| n.to_lowercase().contains(c))
                .unwrap_or(false)
            {
                return false;
            }
        }
        if let Some(s) = &search_lc {
            let in_name = e.name.to_lowercase().contains(s);
            let in_tags = e.tags.iter().any(|t| t.to_lowercase().contains(s));
            if !in_name && !in_tags {
                return false;
            }
        }
        if undownloaded && manifest.contains(e.original_id) {
            return false;
        }
        true
    });

    entries.sort_by(|a, b| b.library_added_at.cmp(&a.library_added_at));
    let shown = entries.len().min(limit);
    println!("{} match{} (showing {shown}):", entries.len(), if entries.len() == 1 { "" } else { "es" });
    for e in entries.iter().take(limit) {
        let mark = if manifest.contains(e.original_id) { "✓" } else { " " };
        let added = e
            .library_added_at
            .as_deref()
            .and_then(|s| s.split('T').next())
            .unwrap_or("?");
        let creator = e.creator_name.as_deref().unwrap_or("?");
        let mlabel = e.month_label().unwrap_or_default();
        println!(
            "  [{mark}] {:>8}  {added}  [{mlabel:>7}]  {}  — {}",
            e.original_id, e.name, creator
        );
    }
    println!("\nDownload with:  minihoard download <id> [<id> ...]");
    Ok(())
}

async fn download(ids: Vec<u64>, keep_archive: bool) -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};

    if ids.is_empty() {
        anyhow::bail!("give one or more object ids, e.g. `minihoard download 806054`");
    }
    let config = Config::load()?;
    let token = mmf_core::auth::access_token(&config.client_id)
        .await
        .context("get access token (run `minihoard login` first)")?;
    let cookie = mmf_core::auth::TokenStore::session_cookie()?;

    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template("  {msg} [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec}")
            .unwrap()
            .progress_chars("=>-"),
    );
    let mut current = String::new();
    let outcomes = mmf_core::pipeline::download_objects(
        &config,
        &token,
        cookie.as_deref(),
        &ids,
        &mmf_core::pipeline::Options { keep_archive },
        |fname, done, total| {
            if fname != current {
                current = fname.to_string();
                pb.set_message(current.clone());
                pb.set_position(0);
            }
            if let Some(t) = total {
                pb.set_length(t);
            }
            pb.set_position(done);
        },
    )
    .await?;
    pb.finish_and_clear();

    for o in &outcomes {
        println!(
            "✓ {} ({} MB, {} files) → {}",
            o.name,
            o.bytes / 1_048_576,
            o.file_count,
            o.dir.display()
        );
    }
    if outcomes.is_empty() {
        println!("Nothing downloaded (do you own the given ids?).");
    } else {
        println!("\nClean releases under {}", config.unpack_dir.display());
    }
    Ok(())
}

fn pack(
    paths: Vec<PathBuf>,
    format: String,
    level: i32,
    split: Option<String>,
    out: Option<PathBuf>,
) -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};
    use mmf_core::pack::{pack_dir, parse_size, PackFormat, PackOptions};

    if paths.is_empty() {
        anyhow::bail!("give one or more folders, e.g. `minihoard pack ~/mmf/Creator-06-2026`");
    }
    let format = PackFormat::parse(&format)?;
    let split_bytes = match split {
        Some(s) => Some(parse_size(&s)?),
        None => None,
    };
    let opts = PackOptions {
        format,
        level,
        split_bytes,
    };

    for src in &paths {
        let src = src.as_path();
        let out_dir = match &out {
            Some(o) => o.clone(),
            None => src.parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf(),
        };

        let pb = ProgressBar::new(0);
        pb.set_style(
            ProgressStyle::with_template("  {msg} [{bar:30}] {bytes}/{total_bytes}")
                .unwrap()
                .progress_chars("=>-"),
        );
        let label = src
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("archive")
            .to_string();
        pb.set_message(label.clone());

        let report = pack_dir(src, &out_dir, &opts, |done| {
            pb.set_position(done);
        })
        .with_context(|| format!("pack {}", src.display()))?;
        pb.set_length(report.input_bytes);
        pb.finish_and_clear();

        let ratio = (report.output_bytes * 100)
            .checked_div(report.input_bytes)
            .map(|pct| 100 - pct)
            .unwrap_or(0);
        let parts = if report.outputs.len() > 1 {
            format!(", {} volumes", report.outputs.len())
        } else {
            String::new()
        };
        println!(
            "✓ {} ({} files, {} MB → {} MB, {}% smaller{}) → {}",
            label,
            report.file_count,
            report.input_bytes / 1_048_576,
            report.output_bytes / 1_048_576,
            ratio,
            parts,
            report.outputs[0].display(),
        );
    }
    Ok(())
}

fn unpack(archive: PathBuf) -> Result<()> {
    let config = Config::load()?;

    if mmf_core::pack::is_tar_zst(&archive) {
        let dest = &config.unpack_dir;
        let n = mmf_core::pack::unpack_tar_zst(&archive, dest)?;
        println!("Unpacked {} files to {}", n, dest.display());
        return Ok(());
    }

    let report = mmf_core::unpack::unpack_zip(&archive, &config.unpack_dir)?;
    println!(
        "Unpacked {} files to {}",
        report.files_written,
        report.dest.display()
    );
    if !report.nested_archives.is_empty() {
        println!("Found {} nested archive(s):", report.nested_archives.len());
        for a in &report.nested_archives {
            println!("  {}", a.display());
        }
    }
    Ok(())
}

async fn sync() -> Result<()> {
    let _config = Config::load()?;
    anyhow::bail!("`sync` arrives in milestone M7");
}

/// Returns the release-asset target triple for the current platform.
fn current_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        (os, arch) => anyhow::bail!("no prebuilt binary for {os}/{arch}"),
    }
}

async fn upgrade() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let target = current_target()?;

    // Fetch latest release tag via GitHub API.
    let client = reqwest::Client::builder()
        .user_agent(concat!("minihoard/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp: serde_json::Value = client.get(&url).send().await?.json().await?;
    let tag = resp["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("unexpected GitHub API response"))?;

    // Tags are "vX.Y.Z"; strip the leading 'v' to compare with CARGO_PKG_VERSION.
    let latest = tag.trim_start_matches('v');
    if latest == current {
        println!("Already up to date (v{current}).");
        return Ok(());
    }
    println!("Upgrading minihoard v{current} → v{latest}…");

    let exe = std::env::current_exe().context("cannot find current executable path")?;
    let bin_dir = exe.parent().context("cannot determine binary directory")?;

    #[cfg(windows)]
    let ext = ".exe";
    #[cfg(not(windows))]
    let ext = "";

    for bin in ["minihoard", "minihoard-mcp"] {
        let url = format!(
            "https://github.com/{REPO}/releases/download/{tag}/{bin}-{target}{ext}"
        );
        print!("  {bin}… ");
        io::stdout().flush()?;

        let bytes = client.get(&url).send().await?.bytes().await?;
        let dest = bin_dir.join(format!("{bin}{ext}"));

        // Write to a sibling temp file, then rename over the target (atomic on Unix;
        // on Windows, renaming over a running exe succeeds because the OS keeps the
        // original file handle open until the process exits).
        let tmp = bin_dir.join(format!(".{bin}.tmp{ext}"));
        std::fs::write(&tmp, &bytes)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }

        std::fs::rename(&tmp, &dest)
            .with_context(|| format!("replace {}", dest.display()))?;
        println!("done");
    }

    println!("Updated to v{latest}.");
    Ok(())
}

fn setup_mcp() -> Result<()> {
    // Locate the minihoard-mcp binary (expected next to the current exe).
    let exe = std::env::current_exe().context("cannot find current executable path")?;
    let bin_dir = exe.parent().context("cannot determine binary directory")?;

    #[cfg(windows)]
    let mcp_bin = bin_dir.join("minihoard-mcp.exe");
    #[cfg(not(windows))]
    let mcp_bin = bin_dir.join("minihoard-mcp");

    if !mcp_bin.exists() {
        anyhow::bail!(
            "minihoard-mcp not found at {}\n\
             Make sure both binaries are in the same directory, or run the installer again.",
            mcp_bin.display()
        );
    }

    let config_path = claude_desktop_config_path()?;

    let mut config: serde_json::Value = if config_path.exists() {
        let s = std::fs::read_to_string(&config_path)
            .with_context(|| format!("read {}", config_path.display()))?;
        serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if config.get("mcpServers").is_none() {
        config["mcpServers"] = serde_json::json!({});
    }
    config["mcpServers"]["minihoard"] = serde_json::json!({
        "command": mcp_bin.display().to_string()
    });

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    println!("Registered minihoard-mcp in Claude Desktop.");
    println!("Config: {}", config_path.display());
    println!("\nRestart Claude Desktop to apply.");
    Ok(())
}

fn claude_desktop_config_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let dirs = directories::UserDirs::new()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
        Ok(dirs
            .home_dir()
            .join("Library/Application Support/Claude/claude_desktop_config.json"))
    }
    #[cfg(target_os = "windows")]
    {
        let appdata =
            std::env::var("APPDATA").context("APPDATA environment variable not set")?;
        Ok(PathBuf::from(appdata).join("Claude/claude_desktop_config.json"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let dirs = directories::UserDirs::new()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
        Ok(dirs
            .home_dir()
            .join(".config/Claude/claude_desktop_config.json"))
    }
}

fn prompt(label: &str, default: Option<String>) -> Result<String> {
    loop {
        match &default {
            Some(d) => print!("{label} [{d}]: "),
            None => print!("{label}: "),
        }
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
        if let Some(d) = &default {
            return Ok(d.clone());
        }
        println!("  (required)");
    }
}

/// Prompt with no default; empty input yields `None`.
fn prompt_optional(label: &str) -> Result<Option<String>> {
    print!("{label} (optional): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    })
}

fn prompt_path(label: &str, default: PathBuf) -> Result<PathBuf> {
    let s = prompt(label, Some(default.display().to_string()))?;
    Ok(PathBuf::from(shellexpand_tilde(&s)))
}

/// Minimal `~` expansion so the wizard accepts `~/foo` paths.
fn shellexpand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
            return home.join(rest).display().to_string();
        }
    }
    s.to_string()
}
