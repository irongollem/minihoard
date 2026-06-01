use std::path::PathBuf;

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// All errors surfaced by `mmf-core`.
///
/// Variants are intentionally coarse: the CLI turns these into user-facing
/// messages, and an eventual MCP facade maps them to structured errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("config file not found at {0} — run `minihoard configure` first")]
    ConfigMissing(PathBuf),

    #[error("not authenticated — run `minihoard login` first")]
    NotAuthenticated,

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("MyMiniFactory API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("download failed: {0}")]
    Download(String),

    #[error("integrity check failed for {file}: expected {expected}, got {actual}")]
    Integrity {
        file: String,
        expected: String,
        actual: String,
    },

    #[error("unpack failed: {0}")]
    Unpack(String),

    #[error("secret storage error: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("toml parse error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),
}
