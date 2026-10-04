//! The Lobotomy backend process. v1 runs one project per instance (data-model.md §10).

use std::path::PathBuf;

use anyhow::bail;
use lobotomyd::project;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let mut args = std::env::args().skip(1);
    let data_dir = match (args.next().as_deref(), args.next()) {
        (Some("--data-dir"), Some(dir)) => PathBuf::from(dir),
        _ => bail!("usage: lobotomyd --data-dir <dir>"),
    };

    let project = project::Project::open(&data_dir)?;
    tracing::info!(data_dir = %project.data_dir.display(), "project opened");
    Ok(())
}
