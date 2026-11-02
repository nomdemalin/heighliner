use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};

use super::config::{Action, Match, Rule, Tier};

pub struct CompiledRule<'a> {
    pub src: &'a Rule,
    pub name_glob: Option<GlobSet>,
    pub path_glob: Option<GlobSet>,
}

pub fn compile(rules: &[Rule]) -> Result<Vec<CompiledRule<'_>>> {
    rules
        .iter()
        .filter(|r| r.enabled.unwrap_or(true))
        .map(|r| {
            Ok(CompiledRule {
                src: r,
                name_glob: build_set(&r.r#match.name_glob)?,
                path_glob: build_set(&r.r#match.path_glob)?,
            })
        })
        .collect()
}

fn build_set(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(Glob::new(p)?);
    }
    Ok(Some(b.build()?))
}

#[derive(Debug, Clone)]
pub struct Candidate<'a> {
    pub path: &'a Path,
    pub size: u64,
    pub tier: Tier,
    pub age: Duration,
}

#[derive(Debug, Clone)]
pub enum Resolved {
    MoveTo(PathBuf),
    Retier(Tier),
    Archive { to: PathBuf, delete_source: bool },
    DeleteAfter(#[allow(dead_code)] Duration),
}

pub fn first_match<'a>(
    rules: &'a [CompiledRule<'a>],
    cand: &Candidate<'a>,
) -> Option<(&'a Rule, Resolved)> {
    for r in rules {
        if matches(cand, r) {
            let action = match &r.src.action {
                Action::Move { to } => Resolved::MoveTo(to.clone()),
                Action::Retier { to } => Resolved::Retier(*to),
                Action::Archive { to, delete_source } => Resolved::Archive {
                    to: to.clone(),
                    delete_source: *delete_source,
                },
                Action::DeleteAfter { after } => Resolved::DeleteAfter(*after),
            };
            return Some((r.src, action));
        }
    }
    None
}

fn matches(c: &Candidate<'_>, r: &CompiledRule<'_>) -> bool {
    let m: &Match = &r.src.r#match;

    if !m.extension.is_empty() {
        let ext = c.path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let ext_lc = ext.to_ascii_lowercase();
        if !m.extension.iter().any(|want| want.eq_ignore_ascii_case(&ext_lc)) {
            return false;
        }
    }
    if let Some(set) = &r.name_glob {
        let name = c.path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !set.is_match(name) {
            return false;
        }
    }
    if let Some(set) = &r.path_glob {
        if !set.is_match(c.path) {
            return false;
        }
    }
    if let Some(prefix) = &m.source_prefix {
        if !c.path.starts_with(prefix) {
            return false;
        }
    }
    if let Some(older) = m.older_than {
        if c.age < older {
            return false;
        }
    }
    if let Some(tier) = m.in_tier {
        if c.tier != tier {
            return false;
        }
    }
    if let Some(min) = m.min_size {
        if c.size < min {
            return false;
        }
    }
    if let Some(max) = m.max_size {
        if c.size > max {
            return false;
        }
    }
    true
}

/// Compute destination path for a Move action: `dest_root/<tier>/<filename>`.
pub fn move_destination(dest_root: &Path, tier: Tier, src: &Path) -> PathBuf {
    let mut p = dest_root.to_path_buf();
    p.push(tier.dir_name());
    if let Some(name) = src.file_name() {
        p.push(name);
    }
    p
}

