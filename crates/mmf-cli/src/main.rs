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
    /// Show the current config and where files are stored (downloads, etc.).
    #[command(visible_alias = "where")]
    Config,
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
        /// Filter by source channel, e.g. tribe / purchase / kickstarter.
        #[arg(long)]
        source: Option<String>,
        /// Only show items not yet downloaded.
        #[arg(long)]
        undownloaded: bool,
        /// Show at most N items (default 60).
        #[arg(long, default_value_t = 60)]
        limit: usize,
    },
    /// Get one or more releases by id or name (downloads, unpacks, cleans,
    /// reorganizes — leaves clean releases on disk).
    #[command(visible_alias = "get")]
    Download {
        /// Object ids (e.g. 806054) or names to search for (e.g. "dragon").
        targets: Vec<String>,
        /// Keep the original .zip after unpacking (default: delete it).
        #[arg(long)]
        keep_archive: bool,
        /// Batch: every release from this month, e.g. 2026-06 (asks first).
        #[arg(long)]
        month: Option<String>,
        /// Batch: every release from this creator (asks first).
        #[arg(long)]
        creator: Option<String>,
        /// Batch: every release whose name/tags match this text (asks first).
        #[arg(long)]
        search: Option<String>,
        /// Batch: every release from this source, e.g. tribe / kickstarter.
        #[arg(long)]
        source: Option<String>,
        /// Batch: restrict the above filters to items not yet downloaded.
        #[arg(long)]
        undownloaded: bool,
        /// Skip the confirmation prompt for batch (filter) downloads.
        #[arg(short = 'y', long)]
        yes: bool,
        /// How many objects to download in parallel (default: from config, ~5).
        #[arg(short = 'j', long)]
        jobs: Option<usize>,
        /// After downloading, repack each touched month group into a tar.zst
        /// archive for backup. Implied by --split / --name.
        #[arg(long)]
        pack: bool,
        /// Split the repack into fixed-size volumes, e.g. 4G (implies --pack).
        #[arg(long)]
        split: Option<String>,
        /// Archive base filename for the repack, verbatim (implies --pack;
        /// only valid when the batch touches a single month group).
        #[arg(long)]
        name: Option<String>,
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
        /// Archive base filename, verbatim (e.g. "Dungeon Classics - 2026-04").
        /// Only valid with a single folder; defaults to the folder name.
        #[arg(long)]
        name: Option<String>,
        /// Don't write the `<archive>.json` content index.
        #[arg(long)]
        no_sidecar: bool,
    },
    /// Tidy existing release folders: strip macOS junk and collapse redundant
    /// single-folder nesting. With no paths, tidies your whole library.
    Tidy {
        /// Folders to tidy (default: every release under the unpack dir).
        paths: Vec<PathBuf>,
    },
    /// Unpack a downloaded archive (.zip or .tar.zst) into the unpack directory.
    Unpack {
        /// Path to a `.zip` or `.tar.zst` archive (a `.001` volume for splits).
        archive: PathBuf,
        /// Delete the archive (all split volumes + sidecar) after a successful
        /// extraction.
        #[arg(long)]
        delete_archive: bool,
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
        Command::Config => show_config(),
        Command::Login => login().await,
        Command::Logout => logout(),
        Command::Whoami => whoami().await,
        Command::SetCookie => set_cookie(),
        Command::Explore { object } => explore(object).await,
        Command::List {
            month,
            creator,
            search,
            source,
            undownloaded,
            limit,
        } => list(month, creator, search, source, undownloaded, limit).await,
        Command::Download {
            targets,
            keep_archive,
            month,
            creator,
            search,
            source,
            undownloaded,
            yes,
            jobs,
            pack,
            split,
            name,
        } => {
            download(
                targets,
                keep_archive,
                jobs,
                DownloadFilters {
                    month,
                    creator,
                    search,
                    source,
                    undownloaded,
                    yes,
                },
                PackAfter { pack, split, name },
            )
            .await
        }
        Command::Pack {
            paths,
            format,
            level,
            split,
            out,
            name,
            no_sidecar,
        } => pack(paths, format, level, split, out, name, no_sidecar),
        Command::Tidy { paths } => tidy(paths),
        Command::Unpack {
            archive,
            delete_archive,
        } => unpack(archive, delete_archive),
        Command::Sync => sync().await,
        Command::Upgrade => upgrade().await,
        Command::SetupMcp => setup_mcp(),
    }
}

/// Interactive first-run wizard. Writes a non-secret config file; secrets are
/// captured later by `login`.
fn show_config() -> Result<()> {
    match Config::load() {
        Ok(cfg) => println!("{}", cfg.describe()),
        Err(mmf_core::Error::ConfigMissing(p)) => {
            println!("No config yet (expected at {}).", p.display());
            println!("Run `minihoard configure` to set it up.");
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

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
    let library_dir = Config::default_library_dir()?;
    println!(
        "\nWhere should your unpacked releases be stored? Pick a visible, user-owned\n\
         folder (your config and download history stay in the app's data dir)."
    );
    let unpack_dir = prompt_path(
        "Library (unpack) directory",
        existing
            .as_ref()
            .map(|c| c.unpack_dir.clone())
            .unwrap_or(library_dir),
    )?;
    // download_dir is legacy (everything lands in the unpack dir now); keep the
    // existing value or default it alongside the library.
    let download_dir = existing
        .as_ref()
        .map(|c| c.download_dir.clone())
        .unwrap_or_else(|| data_dir.join("downloads"));

    let config = Config {
        client_id,
        redirect_port: existing.as_ref().map(|c| c.redirect_port).unwrap_or(8723),
        download_dir,
        unpack_dir,
        download_concurrency: existing
            .as_ref()
            .map(|c| c.download_concurrency)
            .unwrap_or(5),
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
    source: Option<String>,
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

    let any_filter = month.is_some()
        || creator.is_some()
        || search.is_some()
        || source.is_some()
        || undownloaded;

    // No filters → show per-month and per-source overviews to help the user choose.
    if !any_filter {
        let mut by_month: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_source: BTreeMap<String, usize> = BTreeMap::new();
        for e in &entries {
            *by_month
                .entry(e.yearmonth().unwrap_or_else(|| "unknown".into()))
                .or_default() += 1;
            *by_source
                .entry(e.source.clone().unwrap_or_else(|| "unknown".into()))
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
        println!("\nBy source (use --source to filter):");
        let mut sources: Vec<(&String, &usize)> = by_source.iter().collect();
        sources.sort_by(|a, b| b.1.cmp(a.1));
        for (src, n) in sources {
            println!("  {src:>12}  {n:>4}");
        }
        println!("\nShowing newest {limit}. Narrow with --month / --creator / --search / --source / --undownloaded.\n");
    }

    // Apply filters.
    filter_entries(
        &mut entries,
        month.as_deref(),
        creator.as_deref(),
        search.as_deref(),
        source.as_deref(),
        undownloaded,
        &manifest,
    );

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
    println!("\nDownload with:  minihoard get <id|name> [...]  (or a filter like --month/--source)");
    Ok(())
}

/// Retain only library entries matching the given filters (shared by `list`
/// and the filter-batch path of `get`).
fn filter_entries(
    entries: &mut Vec<mmf_core::library::LibraryEntry>,
    month: Option<&str>,
    creator: Option<&str>,
    search: Option<&str>,
    source: Option<&str>,
    undownloaded: bool,
    manifest: &mmf_core::manifest::Manifest,
) {
    let month_norm = month.map(|m| m.replace('-', ""));
    let creator_lc = creator.map(|c| c.to_lowercase());
    let search_lc = search.map(|s| s.to_lowercase());
    let source_lc = source.map(|s| s.to_lowercase());
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
        if let Some(src) = &source_lc {
            if !e
                .source
                .as_deref()
                .map(|s| s.to_lowercase().contains(src))
                .unwrap_or(false)
            {
                return false;
            }
        }
        if undownloaded && manifest.contains(e.original_id) {
            return false;
        }
        true
    });
}

/// Turn a mix of numeric ids and search terms into a deduped list of object
/// ids. Numeric tokens pass through; name tokens are matched (case-insensitive,
/// across name/creator/tags) against the library. A term with several matches
/// prompts an interactive pick; a term with none warns and is skipped.
async fn resolve_targets(targets: &[String], cookie: Option<&str>) -> Result<Vec<u64>> {
    use std::collections::BTreeSet;

    let mut ids: BTreeSet<u64> = BTreeSet::new();
    let mut names: Vec<&str> = Vec::new();
    for t in targets {
        match t.trim().parse::<u64>() {
            Ok(id) => {
                ids.insert(id);
            }
            Err(_) => names.push(t.trim()),
        }
    }
    if names.is_empty() {
        return Ok(ids.into_iter().collect());
    }

    // Names need the library, which is gated on the website session cookie.
    let cookie = cookie.ok_or_else(|| {
        anyhow::anyhow!(
            "searching by name needs your library — run `minihoard set-cookie` first, \
             or pass the numeric id"
        )
    })?;
    let library = mmf_core::library::dedupe(mmf_core::library::fetch_library(cookie).await?);

    for term in names {
        let needle = term.to_lowercase();
        let matches: Vec<&mmf_core::library::LibraryEntry> = library
            .iter()
            .filter(|e| {
                e.name.to_lowercase().contains(&needle)
                    || e.creator_name
                        .as_deref()
                        .is_some_and(|c| c.to_lowercase().contains(&needle))
                    || e.tags.iter().any(|t| t.to_lowercase().contains(&needle))
            })
            .collect();

        match matches.as_slice() {
            [] => eprintln!("No match for \"{term}\" — skipping."),
            [one] => {
                println!("\"{term}\" → {} ({})", one.name, one.original_id);
                ids.insert(one.original_id);
            }
            many => {
                for id in pick_matches(term, many)? {
                    ids.insert(id);
                }
            }
        }
    }
    Ok(ids.into_iter().collect())
}

/// Print numbered matches and let the user choose (numbers, `all`, or blank to
/// skip). Returns the chosen object ids.
fn pick_matches(term: &str, matches: &[&mmf_core::library::LibraryEntry]) -> Result<Vec<u64>> {
    println!("\n\"{term}\" matches {} items:", matches.len());
    for (i, e) in matches.iter().enumerate() {
        let creator = e.creator_name.as_deref().unwrap_or("?");
        let month = e.month_label().unwrap_or_else(|| "—".to_string());
        println!("  [{}] {} — {} ({}) #{}", i + 1, e.name, creator, month, e.original_id);
    }
    let choice = prompt("Pick numbers (e.g. 1 3), `all`, or blank to skip", None)?;
    let choice = choice.trim();
    if choice.is_empty() {
        return Ok(Vec::new());
    }
    if choice.eq_ignore_ascii_case("all") {
        return Ok(matches.iter().map(|e| e.original_id).collect());
    }
    let mut out = Vec::new();
    for tok in choice.split(|c: char| c.is_whitespace() || c == ',') {
        if tok.is_empty() {
            continue;
        }
        let n: usize = tok
            .parse()
            .with_context(|| format!("not a number: {tok}"))?;
        let e = matches
            .get(n - 1)
            .ok_or_else(|| anyhow::anyhow!("out of range: {n}"))?;
        out.push(e.original_id);
    }
    Ok(out)
}

/// Batch filters for `get` — download a whole matching set instead of naming
/// each item. Any field set triggers a confirm-first filter download.
struct DownloadFilters {
    month: Option<String>,
    creator: Option<String>,
    search: Option<String>,
    source: Option<String>,
    undownloaded: bool,
    yes: bool,
}

impl DownloadFilters {
    fn any(&self) -> bool {
        self.month.is_some()
            || self.creator.is_some()
            || self.search.is_some()
            || self.source.is_some()
            || self.undownloaded
    }
}

/// Optional repack step run after a download completes.
struct PackAfter {
    pack: bool,
    split: Option<String>,
    name: Option<String>,
}

impl PackAfter {
    fn enabled(&self) -> bool {
        self.pack || self.split.is_some() || self.name.is_some()
    }
}

async fn download(
    targets: Vec<String>,
    keep_archive: bool,
    jobs: Option<usize>,
    filters: DownloadFilters,
    pack_after: PackAfter,
) -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};
    use std::collections::BTreeSet;

    if targets.is_empty() && !filters.any() {
        anyhow::bail!(
            "give ids/names (e.g. `minihoard get 806054 dragon`) or a batch filter \
             (e.g. `minihoard get --month 2026-06`)"
        );
    }
    let config = Config::load()?;
    let token = mmf_core::auth::access_token(&config.client_id)
        .await
        .context("get access token (run `minihoard login` first)")?;
    let cookie = mmf_core::auth::TokenStore::session_cookie()?;

    let mut id_set: BTreeSet<u64> = resolve_targets(&targets, cookie.as_deref())
        .await?
        .into_iter()
        .collect();

    // Batch: resolve a whole filter set, preview it, and confirm before pulling.
    if filters.any() {
        let cookie = cookie.as_deref().ok_or_else(|| {
            anyhow::anyhow!("batch filters need your library — run `minihoard set-cookie` first")
        })?;
        let mut entries = mmf_core::library::dedupe(
            mmf_core::library::fetch_library(cookie)
                .await
                .context("fetch library")?,
        );
        let manifest = mmf_core::manifest::Manifest::load(&Config::default_data_dir()?)?;
        filter_entries(
            &mut entries,
            filters.month.as_deref(),
            filters.creator.as_deref(),
            filters.search.as_deref(),
            filters.source.as_deref(),
            filters.undownloaded,
            &manifest,
        );
        entries.sort_by(|a, b| b.library_added_at.cmp(&a.library_added_at));

        if entries.is_empty() {
            anyhow::bail!("no items match that filter");
        }
        println!("{} item(s) match this batch:", entries.len());
        for e in entries.iter().take(40) {
            let creator = e.creator_name.as_deref().unwrap_or("?");
            let mlabel = e.month_label().unwrap_or_default();
            println!("  {:>8}  [{mlabel:>7}]  {} — {}", e.original_id, e.name, creator);
        }
        if entries.len() > 40 {
            println!("  … and {} more", entries.len() - 40);
        }
        if !filters.yes && !confirm(&format!("Download all {}?", entries.len()))? {
            println!("Cancelled.");
            return Ok(());
        }
        id_set.extend(entries.iter().map(|e| e.original_id));
    }

    let ids: Vec<u64> = id_set.into_iter().collect();
    if ids.is_empty() {
        anyhow::bail!("nothing matched — try `minihoard list --search <term>` to find ids");
    }
    let concurrency = jobs.unwrap_or(config.download_concurrency as usize).max(1);

    use mmf_core::pipeline::Progress;
    use std::collections::HashMap;

    // One aggregate bar across all concurrent downloads: bytes = bytes already
    // finished + bytes in flight across the parallel objects.
    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template("  {spinner} {msg} • {bytes} {binary_bytes_per_sec}")
            .unwrap(),
    );
    let mut inflight: HashMap<String, u64> = HashMap::new();
    let mut completed: u64 = 0;
    let mut done_count = 0usize;

    let outcomes = mmf_core::pipeline::download_objects(
        &config,
        &token,
        cookie.as_deref(),
        &ids,
        &mmf_core::pipeline::Options {
            keep_archive,
            concurrency,
        },
        |p| match p {
            Progress::ObjectStart { index, total, name } => {
                pb.println(format!("→ [{index}/{total}] {name}"));
            }
            Progress::File { object, done, .. } => {
                inflight.insert(object, done);
                pb.set_position(completed + inflight.values().sum::<u64>());
            }
            Progress::ObjectDone { name, bytes, files } => {
                completed += bytes;
                inflight.remove(&name);
                done_count += 1;
                pb.println(format!("✓ {name} ({} MB, {files} files)", bytes / 1_048_576));
                pb.set_message(format!("{done_count}/{} done", ids.len()));
            }
            Progress::ObjectFailed { name, error } => {
                inflight.remove(&name);
                pb.println(format!("⚠ {name}: {error}"));
            }
        },
    )
    .await?;
    pb.finish_and_clear();

    if outcomes.is_empty() {
        println!("Nothing downloaded (do you own the given ids?).");
        return Ok(());
    }
    println!(
        "\nDone: {} release(s) under {}",
        outcomes.len(),
        config.unpack_dir.display()
    );

    if pack_after.enabled() {
        repack_groups(&config, &outcomes, &pack_after)?;
    }
    Ok(())
}

/// Pack each distinct month-group folder touched by a download into a tar.zst
/// archive (for the `get --pack` flow).
fn repack_groups(
    config: &Config,
    outcomes: &[mmf_core::pipeline::Outcome],
    pack_after: &PackAfter,
) -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};
    use mmf_core::pack::{pack_dir, parse_size, PackFormat, PackOptions};
    use std::collections::BTreeSet;

    let split_bytes = match &pack_after.split {
        Some(s) => Some(parse_size(s)?),
        None => None,
    };
    // The group folders are the parents of each release dir (one level under the
    // unpack dir). Dedupe so a multi-release group is packed once.
    let groups: BTreeSet<PathBuf> = outcomes
        .iter()
        .filter_map(|o| o.dir.parent().map(|p| p.to_path_buf()))
        .filter(|p| p != &config.unpack_dir)
        .collect();
    if groups.is_empty() {
        return Ok(());
    }
    if pack_after.name.is_some() && groups.len() > 1 {
        anyhow::bail!(
            "--name only works when the download is a single month group ({} were touched)",
            groups.len()
        );
    }

    println!("\nRepacking {} group(s):", groups.len());
    for group in &groups {
        let out_dir = group.parent().unwrap_or(&config.unpack_dir);
        let opts = PackOptions {
            format: PackFormat::TarZst,
            level: 19,
            split_bytes,
            write_sidecar: true,
        };
        let label = group.file_name().and_then(|s| s.to_str()).unwrap_or("group").to_string();
        let pb = ProgressBar::new(0);
        pb.set_style(
            ProgressStyle::with_template("  {spinner} packing {msg} • {bytes}")
                .unwrap(),
        );
        pb.set_message(label.clone());
        let report = pack_dir(group, out_dir, &opts, pack_after.name.as_deref(), |done| {
            pb.set_position(done);
        })
        .with_context(|| format!("pack {}", group.display()))?;
        pb.finish_and_clear();

        let parts = if report.outputs.len() > 1 {
            format!(", {} volumes", report.outputs.len())
        } else {
            String::new()
        };
        println!(
            "  📦 {label} ({} files, {} MB → {} MB{}) → {}",
            report.file_count,
            report.input_bytes / 1_048_576,
            report.output_bytes / 1_048_576,
            parts,
            report.outputs[0].display()
        );
    }
    Ok(())
}

fn pack(
    paths: Vec<PathBuf>,
    format: String,
    level: i32,
    split: Option<String>,
    out: Option<PathBuf>,
    name: Option<String>,
    no_sidecar: bool,
) -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};
    use mmf_core::pack::{pack_dir, parse_size, PackFormat, PackOptions};

    if paths.is_empty() {
        anyhow::bail!("give one or more folders, e.g. `minihoard pack ~/mmf/Creator-06-2026`");
    }
    if name.is_some() && paths.len() > 1 {
        anyhow::bail!("--name only works with a single folder (you gave {})", paths.len());
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
        write_sidecar: !no_sidecar,
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

        let report = pack_dir(src, &out_dir, &opts, name.as_deref(), |done| {
            pb.set_position(done);
        })
        .with_context(|| format!("pack {}", src.display()))?;
        pb.set_length(report.input_bytes);
        pb.finish_and_clear();

        let ratio = (report.output_bytes.saturating_mul(100))
            .checked_div(report.input_bytes)
            .map(|pct| 100i64 - pct as i64)
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
        if let Some(s) = &report.sidecar {
            println!("  index: {}", s.display());
        }
    }
    Ok(())
}

fn tidy(paths: Vec<PathBuf>) -> Result<()> {
    let targets = if paths.is_empty() {
        let config = Config::load()?;
        let dirs = mmf_core::clean::library_release_dirs(&config.unpack_dir);
        if dirs.is_empty() {
            println!("Nothing to tidy under {}", config.unpack_dir.display());
            return Ok(());
        }
        dirs
    } else {
        paths
    };

    let mut renamed = 0;
    for t in &targets {
        if !t.is_dir() {
            eprintln!("skip (not a directory): {}", t.display());
            continue;
        }
        let final_dir = mmf_core::clean::tidy_dir(t)
            .with_context(|| format!("tidy {}", t.display()))?;
        if final_dir != *t {
            println!("✓ {} → {}", t.display(), final_dir.display());
            renamed += 1;
        }
    }
    println!(
        "Tidied {} folder(s); {} collapsed.",
        targets.len(),
        renamed
    );
    Ok(())
}

fn unpack(archive: PathBuf, delete_archive: bool) -> Result<()> {
    let config = Config::load()?;

    if mmf_core::pack::is_tar_zst(&archive) {
        let n = mmf_core::pack::unpack_tar_zst(&archive, &config.unpack_dir)?;
        println!("Unpacked {} files to {}", n, config.unpack_dir.display());
    } else {
        let report = mmf_core::unpack::unpack_zip(&archive, &config.unpack_dir)?;
        mmf_core::clean::strip_apple_artifacts(&report.dest);
        let dest = mmf_core::clean::flatten_single_dir(&report.dest)
            .unwrap_or_else(|_| report.dest.clone());
        println!("Unpacked {} files to {}", report.files_written, dest.display());
        if !report.nested_archives.is_empty() {
            println!("Found {} nested archive(s):", report.nested_archives.len());
            for a in &report.nested_archives {
                println!("  {}", a.display());
            }
        }
    }

    if delete_archive {
        let removed = mmf_core::pack::remove_archive_files(&archive)?;
        println!("Removed {removed} archive file(s).");
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

/// Yes/no prompt, defaulting to No (blank or anything but y/yes is false).
fn confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let a = input.trim().to_ascii_lowercase();
    Ok(a == "y" || a == "yes")
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
