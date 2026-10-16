use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use crate::host::Host;
use crate::instance::Instance;

/// One project instance: its [`Instance`], which it derefs to.
pub struct Project {
    pub instance: Arc<Instance>,
}

impl Deref for Project {
    type Target = Instance;

    fn deref(&self) -> &Instance {
        &self.instance
    }
}

impl Project {
    /// Opens the project `id` in `data_dir`, created if missing. A data directory that belongs to
    /// another project is refused.
    pub fn open(id: &str, data_dir: &Path, host: Arc<Host>) -> anyhow::Result<Self> {
        let instance = Instance::open(id, data_dir, host)?;
        if let Some(found) = instance.db.read(lobotomy_core::project::load_project)?
            && found.id != id
        {
            anyhow::bail!("{} holds project {}, not {id}", data_dir.display(), found.id);
        }
        Ok(Self { instance: Arc::new(instance) })
    }
}
