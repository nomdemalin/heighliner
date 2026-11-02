use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Top-level config persisted at `~/.config/heighliner/rules.toml`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default, rename = "watch")]
    pub watches: Vec<Watch>,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub tiers: TierPolicy,
    #[serde(default)]
    pub archive: ArchivePolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct General {
    /// If true, log would-be actions instead of applying them.
    #[serde(default)]
    pub dry_run: bool,
    /// Rescan watched roots every N (uses humantime, e.g. "6h", "1d").
    #[serde(with = "humantime_serde", default = "default_rescan")]
    pub rescan_interval: Duration,
    /// Retier sweep cadence.
    #[serde(with = "humantime_serde", default = "default_retier")]
    pub retier_interval: Duration,
}

fn default_rescan() -> Duration {
    Duration::from_secs(6 * 60 * 60)
}
fn default_retier() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

impl Default for General {
    fn default() -> Self {
        Self {
            dry_run: false,
            rescan_interval: default_rescan(),
            retier_interval: default_retier(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Watch {
    pub path: PathBuf,
    #[serde(default = "truthy")]
    pub recursive: bool,
}

fn truthy() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Rule {
    pub name: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub r#match: Match,
    pub action: Action,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Match {
    /// File extensions without the dot, e.g. ["pdf", "jpg"].
    #[serde(default)]
    pub extension: Vec<String>,
    /// Glob patterns matched against the filename (not the whole path).
    #[serde(default)]
    pub name_glob: Vec<String>,
    /// Glob patterns matched against the full path.
    #[serde(default)]
    pub path_glob: Vec<String>,
    /// Only match files whose source directory contains this path prefix.
    #[serde(default)]
    pub source_prefix: Option<PathBuf>,
    /// Only match files older than this (based on last-access age).
    #[serde(default, with = "humantime_serde::option")]
    pub older_than: Option<Duration>,
    /// Only match files in this tier.
    #[serde(default)]
    pub in_tier: Option<Tier>,
    /// Minimum size (bytes).
    #[serde(default)]
    pub min_size: Option<u64>,
    /// Maximum size (bytes).
    #[serde(default)]
    pub max_size: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Hot,
    Warm,
    Cold,
    Frozen,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Hot, Tier::Warm, Tier::Cold, Tier::Frozen];
    pub fn dir_name(self) -> &'static str {
        match self {
            Tier::Hot => "hot",
            Tier::Warm => "warm",
            Tier::Cold => "cold",
            Tier::Frozen => "frozen",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Move matched files into `to`, auto-placing them in a tier subdir.
    Move { to: PathBuf },
    /// Set the tier (and physically re-shuffle into hot/warm/cold/frozen) under the file's managed destination.
    Retier { to: Tier },
    /// Zip the file into the archive destination and (optionally) remove the source.
    Archive {
        to: PathBuf,
        #[serde(default = "truthy")]
        delete_source: bool,
    },
    /// Delete after `after`.
    DeleteAfter {
        #[serde(with = "humantime_serde")]
        after: Duration,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TierPolicy {
    /// Accessed within this duration → hot.
    #[serde(with = "humantime_serde", default = "default_hot")]
    pub hot_within: Duration,
    /// Hot < age ≤ warm_within → warm.
    #[serde(with = "humantime_serde", default = "default_warm")]
    pub warm_within: Duration,
    /// Warm < age ≤ cold_within → cold. Older → frozen.
    #[serde(with = "humantime_serde", default = "default_cold")]
    pub cold_within: Duration,
}

fn default_hot() -> Duration {
    Duration::from_secs(7 * 24 * 60 * 60)
}
fn default_warm() -> Duration {
    Duration::from_secs(30 * 24 * 60 * 60)
}
fn default_cold() -> Duration {
    Duration::from_secs(90 * 24 * 60 * 60)
}

impl Default for TierPolicy {
    fn default() -> Self {
        Self {
            hot_within: default_hot(),
            warm_within: default_warm(),
            cold_within: default_cold(),
        }
    }
}

impl TierPolicy {
    pub fn tier_for_age(&self, age: Duration) -> Tier {
        if age <= self.hot_within {
            Tier::Hot
        } else if age <= self.warm_within {
            Tier::Warm
        } else if age <= self.cold_within {
            Tier::Cold
        } else {
            Tier::Frozen
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ArchivePolicy {
    pub destination: Option<PathBuf>,
    #[serde(default)]
    pub compress: bool,
    #[serde(default)]
    pub schedule: Option<ArchiveSchedule>,
    /// Delete files from `frozen/` after this duration.
    #[serde(default, with = "humantime_serde::option")]
    pub purge_frozen_after: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveSchedule {
    Daily,
    Weekly,
    Monthly,
}

impl Config {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(s) => {
                let cfg: Self = toml::from_str(&s)
                    .with_context(|| format!("parse {}", path.display()))?;
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Self::starter();
                cfg.save(path)?;
                Ok(cfg)
            }
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let s = toml::to_string_pretty(self).context("serialize config")?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, s).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
        Ok(())
    }

    fn starter() -> Self {
        Self {
            general: General::default(),
            watches: vec![],
            rules: vec![],
            tiers: TierPolicy::default(),
            archive: ArchivePolicy::default(),
        }
    }
}
