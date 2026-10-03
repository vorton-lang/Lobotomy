use std::path::{Path, PathBuf};

use anyhow::Context;
use lobotomy_core::Db;

/// One project instance. Everything project-scoped hangs off this value and is passed explicitly;
/// there are no global singletons, so several instances can share one process later
/// (data-model.md §10).
pub struct Project {
    pub data_dir: PathBuf,
    pub db: Db,
}

impl Project {
    pub fn open(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
        let db = Db::open(&data_dir.join("lobotomy.db")).context("opening the project database")?;
        Ok(Self { data_dir: data_dir.to_path_buf(), db })
    }
}
