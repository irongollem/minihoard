//! Local record of what's already been downloaded, so `list` can mark items
//! and `sync`/`download` can skip or report them. Stored as JSON in the data
//! directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// Keyed by object original_id.
    pub objects: BTreeMap<u64, Record>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    /// Unix seconds when downloaded.
    pub downloaded_at: u64,
    /// Filenames written for this object.
    pub files: Vec<String>,
}

impl Manifest {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("manifest.json")
    }

    pub fn load(data_dir: &Path) -> Result<Self> {
        let path = Self::path(data_dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&text).unwrap_or_default())
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(Self::path(data_dir), text)?;
        Ok(())
    }

    pub fn contains(&self, original_id: u64) -> bool {
        self.objects.contains_key(&original_id)
    }

    pub fn record(&mut self, original_id: u64, name: &str, files: Vec<String>, now_unix: u64) {
        self.objects.insert(
            original_id,
            Record {
                name: name.to_string(),
                downloaded_at: now_unix,
                files,
            },
        );
    }
}
