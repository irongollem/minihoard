use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use mmf_core::config::Config;

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
    /// Probe authenticated endpoints to discover your library (M0 spike).
    Explore {
        /// Optional object id you OWN, to confirm files + download_url appear.
        #[arg(long)]
        object: Option<u64>,
    },
    /// List the releases available in your library.
    List,
    /// Download one or more releases by id.
    Download {
        /// Object ids to download. Omit to download everything new.
        ids: Vec<u64>,
    },
    /// Unpack a downloaded archive into the unpack directory.
    Unpack {
        /// Path to a `.zip` archive.
        archive: PathBuf,
    },
    /// Run the full monthly flow: list -> download new -> unpack.
    Sync,
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
        Command::Explore { object } => explore(object).await,
        Command::List => list().await,
        Command::Download { ids } => download(ids).await,
        Command::Unpack { archive } => unpack(archive),
        Command::Sync => sync().await,
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

async fn list() -> Result<()> {
    let _config = Config::load()?;
    anyhow::bail!("`list` arrives in milestone M4");
}

async fn download(ids: Vec<u64>) -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};

    if ids.is_empty() {
        anyhow::bail!("give one or more object ids, e.g. `minihoard download 806054`");
    }
    let config = Config::load()?;
    let token = mmf_core::auth::access_token(&config.client_id)
        .await
        .context("get access token (run `minihoard login` first)")?;
    let client = mmf_core::api::Client::with_bearer(token.clone());

    for id in ids {
        let object = client
            .get(&format!("/objects/{id}"))
            .await
            .with_context(|| format!("fetch object {id}"))?;
        let name = object["name"].as_str().unwrap_or("object");
        let files = object["files"]["items"].as_array().cloned().unwrap_or_default();

        if files.is_empty() {
            println!("object {id} ({name}): no downloadable files (do you own it?)");
            continue;
        }
        println!("object {id}: {name} — {} file(s)", files.len());
        let dest_dir = config.download_dir.join(id.to_string());

        for file in files {
            let Some(url) = file["download_url"].as_str() else {
                println!("  skipping a file with no download_url (not owned?)");
                continue;
            };
            let filename = file["filename"].as_str().unwrap_or("download.bin");
            let dest = dest_dir.join(filename);

            let pb = ProgressBar::new(0);
            pb.set_style(
                ProgressStyle::with_template(
                    "  {msg} [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec}",
                )
                .unwrap()
                .progress_chars("=>-"),
            );
            pb.set_message(filename.to_string());

            let report = mmf_core::download::download_to(url, &dest, &token, |done, total| {
                if let Some(t) = total {
                    pb.set_length(t);
                }
                pb.set_position(done);
            })
            .await
            .with_context(|| format!("download {filename}"))?;

            pb.finish_with_message(format!(
                "{filename} ({} MB{})",
                report.bytes / 1_048_576,
                if report.resumed { ", resumed" } else { "" }
            ));
        }
    }
    println!("Done. Files in {}", config.download_dir.display());
    Ok(())
}

fn unpack(archive: PathBuf) -> Result<()> {
    let config = Config::load()?;
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
