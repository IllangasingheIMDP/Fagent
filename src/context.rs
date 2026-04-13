use std::path::Path;

use jwalk::WalkDir;
use serde::Serialize;

use crate::{FagentError, Result};

const MAX_ENTRIES: usize = 256;
const MAX_JSON_BYTES: usize = 24_000;

#[derive(Debug, Clone, Serialize)]
pub struct DirectoryContext {
    pub root: String,
    pub depth: usize,
    pub truncated: bool,
    pub entries: Vec<ContextEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextEntry {
    pub path: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

pub fn scan_workspace(root: &Path, depth: usize) -> Result<DirectoryContext> {
    let mut entries = Vec::new();
    for entry in WalkDir::new(root)
        .max_depth(depth.saturating_add(1))
        .sort(true)
    {
        let entry = entry.map_err(|error| {
            FagentError::Execution(format!("failed to scan workspace: {error}"))
        })?;

        if entry.depth() == 0 {
            continue;
        }

        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| FagentError::Execution(format!("invalid scan result: {error}")))?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = entry.metadata().ok();
        let kind = if entry.file_type().is_dir() {
            EntryKind::Directory
        } else if entry.file_type().is_file() {
            EntryKind::File
        } else if entry.file_type().is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::Other
        };
        let size = metadata
            .as_ref()
            .and_then(|metadata| metadata.is_file().then_some(metadata.len()));
        let hint = metadata
            .as_ref()
            .and_then(|metadata| metadata.is_file().then_some(()))
            .and_then(|_| infer_hint(&path));

        entries.push(ContextEntry {
            path: relative,
            kind,
            size,
            hint,
        });
    }

    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let mut context = DirectoryContext {
        root: root.display().to_string(),
        depth,
        truncated: false,
        entries,
    };
    context.truncate_to_limits()?;
    Ok(context)
}

fn infer_hint(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if file_name == "cargo.toml" {
        return Some("rust-manifest".into());
    }
    if file_name == "package.json" {
        return Some("node-manifest".into());
    }
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
}

impl DirectoryContext {
    pub fn to_compact_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(Into::into)
    }

    fn truncate_to_limits(&mut self) -> Result<()> {
        if self.entries.len() > MAX_ENTRIES {
            self.entries.truncate(MAX_ENTRIES);
            self.truncated = true;
        }

        while serde_json::to_vec(self)?.len() > MAX_JSON_BYTES && !self.entries.is_empty() {
            self.entries.pop();
            self.truncated = true;
        }

        Ok(())
    }
}
