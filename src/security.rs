use std::path::{Component, Path, PathBuf};

use crate::{FagentError, Result};

#[derive(Debug, Clone)]
pub struct WorkspacePolicy {
    root: PathBuf,
    canonical_root: PathBuf,
    pub allow_global: bool,
    pub permanent_delete: bool,
}

impl WorkspacePolicy {
    pub fn new(root: PathBuf, allow_global: bool, permanent_delete: bool) -> Result<Self> {
        let canonical_root = dunce::canonicalize(&root)?;
        Ok(Self {
            root,
            canonical_root,
            allow_global,
            permanent_delete,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve_path(&self, raw: &str) -> Result<PathBuf> {
        if raw.trim().is_empty() {
            return Err(FagentError::Validation("paths cannot be empty".into()));
        }

        let candidate = self.to_candidate_path(raw)?;
        validate_windows_reserved_names(&candidate)?;
        let resolved = canonicalize_allow_missing(&candidate)?;

        if !self.allow_global && !path_starts_with(&resolved, &self.canonical_root) {
            return Err(FagentError::Validation(format!(
                "path escapes the workspace jail: {raw}"
            )));
        }

        Ok(resolved)
    }

    pub fn display_path(&self, path: &Path) -> String {
        if path_starts_with(path, &self.canonical_root) {
            if let Ok(relative) = path.strip_prefix(&self.canonical_root) {
                return relative.to_string_lossy().replace('\\', "/");
            }
        }
        path.display().to_string()
    }

    pub fn path_key(&self, path: &Path) -> String {
        path_components(path)
            .into_iter()
            .map(normalize_component)
            .collect::<Vec<_>>()
            .join("/")
    }

    fn to_candidate_path(&self, raw: &str) -> Result<PathBuf> {
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            if !self.allow_global {
                return Err(FagentError::Validation(format!(
                    "absolute paths require --allow-global: {raw}"
                )));
            }
            Ok(path)
        } else {
            Ok(self.root.join(path))
        }
    }
}

pub fn canonicalize_allow_missing(path: &Path) -> Result<PathBuf> {
    let mut missing = Vec::new();
    let mut cursor = path.to_path_buf();

    while !cursor.exists() {
        let file_name = cursor.file_name().ok_or_else(|| {
            FagentError::Validation(format!(
                "could not resolve path because no existing ancestor was found: {}",
                path.display()
            ))
        })?;
        missing.push(file_name.to_owned());
        cursor = cursor
            .parent()
            .ok_or_else(|| {
                FagentError::Validation(format!(
                    "could not resolve path because no existing ancestor was found: {}",
                    path.display()
                ))
            })?
            .to_path_buf();
    }

    let mut resolved = dunce::canonicalize(&cursor)?;
    for part in missing.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn validate_windows_reserved_names(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        const RESERVED: &[&str] = &[
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];

        for component in path_components(path) {
            let uppercase = component
                .trim_end_matches('.')
                .trim_end_matches(' ')
                .to_ascii_uppercase();
            if RESERVED.contains(&uppercase.as_str()) {
                return Err(FagentError::Validation(format!(
                    "reserved Windows device name is not allowed: {component}"
                )));
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = path;
    }

    Ok(())
}

fn path_starts_with(path: &Path, base: &Path) -> bool {
    #[cfg(windows)]
    {
        let path_parts = path_components(path)
            .into_iter()
            .map(|component| component.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let base_parts = path_components(base)
            .into_iter()
            .map(|component| component.to_ascii_lowercase())
            .collect::<Vec<_>>();
        path_parts.starts_with(&base_parts)
    }

    #[cfg(not(windows))]
    {
        path.starts_with(base)
    }
}

fn path_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().into_owned()),
            Component::RootDir => Some(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => None,
            Component::ParentDir => Some("..".into()),
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
        })
        .collect()
}

fn normalize_component(component: String) -> String {
    #[cfg(windows)]
    {
        component.to_ascii_lowercase()
    }

    #[cfg(not(windows))]
    {
        component
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::WorkspacePolicy;

    #[test]
    fn traversal_is_blocked() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace.clone(), false, false).unwrap();

        let result = policy.resolve_path("../outside.txt");
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_blocked() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, workspace.join("escape")).unwrap();

        let policy = WorkspacePolicy::new(workspace.clone(), false, false).unwrap();
        let result = policy.resolve_path("escape/file.txt");
        assert!(result.is_err());
    }

    #[test]
    fn path_key_honors_platform_case_rules() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace.clone(), false, false).unwrap();
        let upper = workspace.join("Reports").join("FILE.txt");
        let lower = workspace.join("reports").join("file.txt");

        #[cfg(windows)]
        assert_eq!(policy.path_key(&upper), policy.path_key(&lower));

        #[cfg(not(windows))]
        assert_ne!(policy.path_key(&upper), policy.path_key(&lower));
    }
}
