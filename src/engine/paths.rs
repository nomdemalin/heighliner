use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub index_file: PathBuf,
    pub log_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .context("no config dir on this platform")?
            .join("Heighliner");
        let data_dir = dirs::data_dir()
            .context("no data dir on this platform")?
            .join("Heighliner");

        Ok(Self {
            config_file: config_dir.join("rules.toml"),
            config_dir,
            index_file: data_dir.join("index.sqlite"),
            log_dir: data_dir.join("logs"),
            data_dir,
        })
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for p in [&self.config_dir, &self.data_dir, &self.log_dir] {
            fs::create_dir_all(p).with_context(|| format!("create {}", p.display()))?;
        }
        Ok(())
    }
}
