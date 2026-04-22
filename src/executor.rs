use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use tokio::fs;
use tracing::info;

use crate::plan::{EffectiveActionKind, ValidatedPlan};
use crate::security::WorkspacePolicy;
use crate::{FagentError, Result};

const DEFAULT_MAX_UNZIP_ENTRIES: usize = 10_000;
const DEFAULT_MAX_UNZIP_TOTAL_BYTES: u64 = 1_000_000_000;
const DEFAULT_MAX_UNZIP_ENTRY_BYTES: u64 = 200_000_000;
const DEFAULT_MAX_UNZIP_COMPRESSION_RATIO: u64 = 500;
const DEFAULT_MAX_UNZIP_PATH_DEPTH: usize = 25;

#[derive(Debug, Clone, Copy)]
struct UnzipLimits {
    max_entries: usize,
    max_total_uncompressed_bytes: u64,
    max_entry_uncompressed_bytes: u64,
    max_compression_ratio: u64,
    max_path_depth: usize,
}

const DEFAULT_UNZIP_LIMITS: UnzipLimits = UnzipLimits {
    max_entries: DEFAULT_MAX_UNZIP_ENTRIES,
    max_total_uncompressed_bytes: DEFAULT_MAX_UNZIP_TOTAL_BYTES,
    max_entry_uncompressed_bytes: DEFAULT_MAX_UNZIP_ENTRY_BYTES,
    max_compression_ratio: DEFAULT_MAX_UNZIP_COMPRESSION_RATIO,
    max_path_depth: DEFAULT_MAX_UNZIP_PATH_DEPTH,
};

#[derive(Debug, Clone)]
pub struct ExecutionFailure {
    pub action_id: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ExecutionReport {
    pub completed: Vec<String>,
    pub failed: Option<ExecutionFailure>,
    pub pending: Vec<String>,
}

impl ExecutionReport {
    pub fn succeeded(&self) -> bool {
        self.failed.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct Executor {
    policy: WorkspacePolicy,
}

impl Executor {
    pub fn new(policy: WorkspacePolicy) -> Self {
        Self { policy }
    }

    pub async fn run(&self, plan: &ValidatedPlan) -> ExecutionReport {
        let mut completed = Vec::new();

        for (index, action) in plan.actions.iter().enumerate() {
            info!("executing action {}", action.id);
            let result = match action.effective_kind {
                EffectiveActionKind::CreateDir => {
                    self.create_dir(action.destination.as_ref().expect("validated"))
                        .await
                }
                EffectiveActionKind::CreateFile => {
                    self.create_file(
                        action.destination.as_ref().expect("validated"),
                        action.content.as_deref().expect("validated"),
                    )
                    .await
                }
                EffectiveActionKind::MoveFile | EffectiveActionKind::RenamePath => {
                    self.move_path(
                        action.source.as_ref().expect("validated"),
                        action.destination.as_ref().expect("validated"),
                    )
                    .await
                }
                EffectiveActionKind::ZipPath => {
                    self.zip_path(
                        action.source.as_ref().expect("validated"),
                        action.destination.as_ref().expect("validated"),
                    )
                    .await
                }
                EffectiveActionKind::UnzipArchive => {
                    self.unzip_archive(
                        action.source.as_ref().expect("validated"),
                        action.destination.as_ref().expect("validated"),
                    )
                    .await
                }
                EffectiveActionKind::DeleteToTrash => {
                    self.delete_to_trash(action.source.as_ref().expect("validated"))
                        .await
                }
                EffectiveActionKind::DeletePermanent => {
                    self.delete_permanent(action.source.as_ref().expect("validated"))
                        .await
                }
            };

            match result {
                Ok(()) => completed.push(action.id.clone()),
                Err(error) => {
                    return ExecutionReport {
                        completed,
                        failed: Some(ExecutionFailure {
                            action_id: action.id.clone(),
                            message: error.to_string(),
                        }),
                        pending: plan.actions[index + 1..]
                            .iter()
                            .map(|pending| pending.id.clone())
                            .collect(),
                    };
                }
            }
        }

        ExecutionReport {
            completed,
            failed: None,
            pending: Vec::new(),
        }
    }

    async fn create_dir(&self, destination: &Path) -> Result<()> {
        fs::create_dir_all(destination).await?;
        Ok(())
    }

    async fn create_file(&self, destination: &Path, content: &str) -> Result<()> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(destination, content).await?;
        Ok(())
    }

    async fn move_path(&self, source: &Path, destination: &Path) -> Result<()> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }

        match fs::rename(source, destination).await {
            Ok(()) => Ok(()),
            Err(error) if is_cross_device_error(&error) => {
                self.copy_then_remove(source.to_path_buf(), destination.to_path_buf())
                    .await
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn zip_path(&self, source: &Path, destination: &Path) -> Result<()> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }

        let source = source.to_path_buf();
        let destination = destination.to_path_buf();
        tokio::task::spawn_blocking(move || zip_path_sync(&source, &destination)).await??;
        Ok(())
    }

    async fn unzip_archive(&self, source: &Path, destination: &Path) -> Result<()> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }

        let source = source.to_path_buf();
        let destination = destination.to_path_buf();
        tokio::task::spawn_blocking(move || unzip_archive_sync(&source, &destination)).await??;
        Ok(())
    }

    async fn delete_to_trash(&self, source: &Path) -> Result<()> {
        let source = source.to_path_buf();
        tokio::task::spawn_blocking(move || {
            trash::delete(&source)
                .map_err(|error| FagentError::Execution(format!("trash failed: {error}")))
        })
        .await??;
        Ok(())
    }

    async fn delete_permanent(&self, source: &Path) -> Result<()> {
        if source.is_dir() {
            fs::remove_dir_all(source).await?;
        } else {
            fs::remove_file(source).await?;
        }
        Ok(())
    }

    async fn copy_then_remove(&self, source: PathBuf, destination: PathBuf) -> Result<()> {
        let source_for_copy = source.clone();
        let destination_for_copy = destination.clone();
        tokio::task::spawn_blocking(move || {
            copy_path_sync(&source_for_copy, &destination_for_copy)
        })
        .await??;
        if source.is_dir() {
            fs::remove_dir_all(&source).await?;
        } else {
            fs::remove_file(&source).await?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn policy(&self) -> &WorkspacePolicy {
        &self.policy
    }
}

fn is_cross_device_error(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(18) | Some(17))
}

fn zip_path_sync(source: &Path, destination: &Path) -> Result<()> {
    let output = std::fs::File::create(destination)?;
    let mut writer = zip::ZipWriter::new(output);

    if source.is_dir() {
        let root_name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "archive".into());
        add_path_to_zip(&mut writer, source, source, &root_name)?;
    } else {
        let entry_name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| {
                FagentError::Execution(format!(
                    "zip failed: source has no file name: {}",
                    source.display()
                ))
            })?;
        add_file_to_zip(&mut writer, source, &entry_name)?;
    }

    writer
        .finish()
        .map_err(|error| FagentError::Execution(format!("zip finalize failed: {error}")))?;
    Ok(())
}

fn add_path_to_zip(
    writer: &mut zip::ZipWriter<std::fs::File>,
    path: &Path,
    base: &Path,
    root_name: &str,
) -> Result<()> {
    if path.is_dir() {
        let relative = path.strip_prefix(base).map_err(|error| {
            FagentError::Execution(format!("zip failed while building directory entry: {error}"))
        })?;
        let archive_path = if relative.as_os_str().is_empty() {
            PathBuf::from(root_name)
        } else {
            Path::new(root_name).join(relative)
        };

        let mut directory_name = normalize_zip_path(&archive_path);
        directory_name.push('/');
        writer
            .add_directory(directory_name, zip::write::FileOptions::default())
            .map_err(|error| {
                FagentError::Execution(format!("zip failed while adding directory: {error}"))
            })?;

        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            add_path_to_zip(writer, &entry.path(), base, root_name)?;
        }
        return Ok(());
    }

    let relative = path.strip_prefix(base).map_err(|error| {
        FagentError::Execution(format!("zip failed while building file entry: {error}"))
    })?;
    let archive_path = Path::new(root_name).join(relative);
    let entry_name = normalize_zip_path(&archive_path);
    add_file_to_zip(writer, path, &entry_name)
}

fn add_file_to_zip(
    writer: &mut zip::ZipWriter<std::fs::File>,
    source: &Path,
    entry_name: &str,
) -> Result<()> {
    writer
        .start_file(
            entry_name,
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(|error| FagentError::Execution(format!("zip failed while adding file: {error}")))?;

    let mut input = std::fs::File::open(source)?;
    let mut buffer = Vec::new();
    input.read_to_end(&mut buffer)?;
    writer.write_all(&buffer)?;
    Ok(())
}

fn normalize_zip_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn unzip_archive_sync(source: &Path, destination: &Path) -> Result<()> {
    unzip_archive_sync_with_limits(source, destination, DEFAULT_UNZIP_LIMITS)
}

fn unzip_archive_sync_with_limits(
    source: &Path,
    destination: &Path,
    limits: UnzipLimits,
) -> Result<()> {
    let input = std::fs::File::open(source)?;
    let mut archive = zip::ZipArchive::new(input)
        .map_err(|error| FagentError::Execution(format!("unzip failed to open archive: {error}")))?;

    if archive.len() > limits.max_entries {
        return Err(FagentError::Execution(format!(
            "unzip rejected archive with too many entries: {} (limit={})",
            archive.len(),
            limits.max_entries
        )));
    }

    std::fs::create_dir_all(destination)?;
    let mut total_uncompressed = 0_u64;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            FagentError::Execution(format!("unzip failed to read archive entry: {error}"))
        })?;

        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            if file_type == 0o120000 {
                return Err(FagentError::Execution(format!(
                    "unzip rejected symlink entry: {}",
                    entry.name()
                )));
            }

            if file_type != 0 && file_type != 0o040000 && file_type != 0o100000 {
                return Err(FagentError::Execution(format!(
                    "unzip rejected special file entry: {}",
                    entry.name()
                )));
            }
        }

        let declared_size = entry.size();
        if declared_size > limits.max_entry_uncompressed_bytes {
            return Err(FagentError::Execution(format!(
                "unzip rejected oversized entry {} bytes for {} (limit={})",
                declared_size,
                entry.name(),
                limits.max_entry_uncompressed_bytes
            )));
        }

        let compressed_size = entry.compressed_size();
        if compressed_size == 0 {
            if declared_size > 0 {
                return Err(FagentError::Execution(format!(
                    "unzip rejected suspicious compressed size for {}",
                    entry.name()
                )));
            }
        } else if (declared_size as u128)
            > (compressed_size as u128) * (limits.max_compression_ratio as u128)
        {
            return Err(FagentError::Execution(format!(
                "unzip rejected high compression ratio for {} ({} / {} > {})",
                entry.name(),
                declared_size,
                compressed_size,
                limits.max_compression_ratio
            )));
        }

        let enclosed = entry.enclosed_name().map(|path| path.to_path_buf()).ok_or_else(|| {
            FagentError::Execution(format!(
                "unzip rejected unsafe archive entry path: {}",
                entry.name()
            ))
        })?;
        if enclosed.components().count() > limits.max_path_depth {
            return Err(FagentError::Execution(format!(
                "unzip rejected deep archive path for {} (depth limit={})",
                entry.name(),
                limits.max_path_depth
            )));
        }
        let output = destination.join(enclosed);

        if entry.name().ends_with('/') {
            std::fs::create_dir_all(&output)?;
            continue;
        }

        if output.exists() {
            return Err(FagentError::Execution(format!(
                "unzip would overwrite existing path: {}",
                output.display()
            )));
        }

        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::File::create(&output)?;
        let entry_name = entry.name().to_string();
        let written = copy_with_limit(
            &mut entry,
            &mut file,
            limits.max_entry_uncompressed_bytes,
            &format!("entry {entry_name}"),
        )?;
        total_uncompressed = total_uncompressed.checked_add(written).ok_or_else(|| {
            FagentError::Execution("unzip rejected archive because extracted size overflowed".into())
        })?;
        if total_uncompressed > limits.max_total_uncompressed_bytes {
            return Err(FagentError::Execution(format!(
                "unzip rejected archive because total extracted size exceeds limit: {} > {}",
                total_uncompressed,
                limits.max_total_uncompressed_bytes
            )));
        }

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let safe_mode = mode & 0o777;
            std::fs::set_permissions(&output, std::fs::Permissions::from_mode(safe_mode))?;
        }
    }

    Ok(())
}

fn copy_with_limit<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    max_bytes: u64,
    label: &str,
) -> Result<u64> {
    let mut total = 0_u64;
    let mut buffer = [0_u8; 8192];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            FagentError::Execution(format!(
                "unzip rejected archive because extracted size overflowed for {label}"
            ))
        })?;
        if total > max_bytes {
            return Err(FagentError::Execution(format!(
                "unzip rejected {label} because extracted bytes exceed limit: {} > {}",
                total, max_bytes
            )));
        }
        writer.write_all(&buffer[..read])?;
    }

    Ok(total)
}

fn copy_path_sync(source: &Path, destination: &Path) -> Result<()> {
    let metadata = std::fs::metadata(source)?;
    if metadata.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let child_source = entry.path();
            let child_destination = destination.join(entry.file_name());
            copy_path_sync(&child_source, &child_destination)?;
        }
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, destination)?;
    preserve_unix_permissions(source, destination)?;
    Ok(())
}

fn preserve_unix_permissions(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(source)?.permissions().mode();
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(destination, permissions)?;
    }

    #[cfg(not(unix))]
    {
        let _ = (source, destination);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};

    use tempfile::tempdir;

    use super::{Executor, UnzipLimits, unzip_archive_sync_with_limits};
    use crate::plan::{ActionKind, EffectiveActionKind, ValidatedAction, ValidatedPlan};
    use crate::security::WorkspacePolicy;

    #[tokio::test]
    async fn move_creates_missing_parent_directories() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("note.txt"), "hello").unwrap();
        let policy = WorkspacePolicy::new(workspace.clone(), false, false).unwrap();
        let executor = Executor::new(policy);
        let source = workspace.join("note.txt");
        let destination = workspace.join("archive").join("note.txt");
        let plan = ValidatedPlan {
            workspace_root: workspace.clone(),
            warnings: vec![],
            actions: vec![ValidatedAction {
                id: "1".into(),
                kind: ActionKind::MoveFile,
                effective_kind: EffectiveActionKind::MoveFile,
                source: Some(source),
                destination: Some(destination.clone()),
                content: None,
                display_source: Some("note.txt".into()),
                display_destination: Some("archive/note.txt".into()),
                rationale: None,
                warnings: vec![],
            }],
        };

        let report = executor.run(&plan).await;
        assert!(report.succeeded());
        assert!(destination.exists());
    }

    #[tokio::test]
    async fn fail_fast_reports_remaining_actions() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace.clone(), false, true).unwrap();
        let executor = Executor::new(policy);
        let plan = ValidatedPlan {
            workspace_root: workspace.clone(),
            warnings: vec![],
            actions: vec![
                ValidatedAction {
                    id: "1".into(),
                    kind: ActionKind::DeletePath,
                    effective_kind: EffectiveActionKind::DeletePermanent,
                    source: Some(workspace.join("missing.txt")),
                    destination: None,
                    content: None,
                    display_source: Some("missing.txt".into()),
                    display_destination: None,
                    rationale: None,
                    warnings: vec![],
                },
                ValidatedAction {
                    id: "2".into(),
                    kind: ActionKind::CreateDir,
                    effective_kind: EffectiveActionKind::CreateDir,
                    source: None,
                    destination: Some(workspace.join("later")),
                    content: None,
                    display_source: None,
                    display_destination: Some("later".into()),
                    rationale: None,
                    warnings: vec![],
                },
            ],
        };

        let report = executor.run(&plan).await;
        assert!(!report.succeeded());
        assert_eq!(report.pending, vec!["2"]);
        assert!(!workspace.join("later").exists());
    }

    #[tokio::test]
    async fn create_file_writes_content() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace.clone(), false, false).unwrap();
        let executor = Executor::new(policy);
        let destination = workspace.join("scripts").join("hello.bat");
        let content = "@echo off\r\necho hello\r\n";
        let plan = ValidatedPlan {
            workspace_root: workspace.clone(),
            warnings: vec![],
            actions: vec![ValidatedAction {
                id: "1".into(),
                kind: ActionKind::CreateFile,
                effective_kind: EffectiveActionKind::CreateFile,
                source: None,
                destination: Some(destination.clone()),
                content: Some(content.into()),
                display_source: None,
                display_destination: Some("scripts/hello.bat".into()),
                rationale: None,
                warnings: vec![],
            }],
        };

        let report = executor.run(&plan).await;
        assert!(report.succeeded());
        assert_eq!(fs::read_to_string(destination).unwrap(), content);
    }

    #[tokio::test]
    async fn zip_path_creates_archive() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(workspace.join("docs").join("nested")).unwrap();
        fs::write(workspace.join("docs").join("note.txt"), "hello").unwrap();
        fs::write(
            workspace.join("docs").join("nested").join("deep.txt"),
            "world",
        )
        .unwrap();

        let policy = WorkspacePolicy::new(workspace.clone(), false, false).unwrap();
        let executor = Executor::new(policy);
        let destination = workspace.join("docs.zip");
        let plan = ValidatedPlan {
            workspace_root: workspace.clone(),
            warnings: vec![],
            actions: vec![ValidatedAction {
                id: "1".into(),
                kind: ActionKind::ZipPath,
                effective_kind: EffectiveActionKind::ZipPath,
                source: Some(workspace.join("docs")),
                destination: Some(destination.clone()),
                content: None,
                display_source: Some("docs".into()),
                display_destination: Some("docs.zip".into()),
                rationale: None,
                warnings: vec![],
            }],
        };

        let report = executor.run(&plan).await;
        assert!(report.succeeded());

        let file = fs::File::open(destination).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut extracted = String::new();
        archive
            .by_name("docs/nested/deep.txt")
            .unwrap()
            .read_to_string(&mut extracted)
            .unwrap();
        assert_eq!(extracted, "world");
    }

    #[tokio::test]
    async fn unzip_archive_extracts_files() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let archive_path = workspace.join("bundle.zip");
        {
            let file = fs::File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    "bundle/readme.txt",
                    zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(b"hello zip").unwrap();
            writer.finish().unwrap();
        }

        let policy = WorkspacePolicy::new(workspace.clone(), false, false).unwrap();
        let executor = Executor::new(policy);
        let destination = workspace.join("unzipped");
        let plan = ValidatedPlan {
            workspace_root: workspace.clone(),
            warnings: vec![],
            actions: vec![ValidatedAction {
                id: "1".into(),
                kind: ActionKind::UnzipArchive,
                effective_kind: EffectiveActionKind::UnzipArchive,
                source: Some(archive_path),
                destination: Some(destination.clone()),
                content: None,
                display_source: Some("bundle.zip".into()),
                display_destination: Some("unzipped".into()),
                rationale: None,
                warnings: vec![],
            }],
        };

        let report = executor.run(&plan).await;
        assert!(report.succeeded());
        assert_eq!(
            fs::read_to_string(destination.join("bundle").join("readme.txt")).unwrap(),
            "hello zip"
        );
    }

    #[test]
    fn unzip_rejects_high_compression_ratio() {
        let temp = tempdir().unwrap();
        let archive_path = temp.path().join("bomb.zip");
        {
            let file = fs::File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    "bomb.txt",
                    zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            let payload = vec![b'A'; 512 * 1024];
            writer.write_all(&payload).unwrap();
            writer.finish().unwrap();
        }

        let destination = temp.path().join("out");
        let error = unzip_archive_sync_with_limits(
            &archive_path,
            &destination,
            UnzipLimits {
                max_entries: 100,
                max_total_uncompressed_bytes: 1_000_000,
                max_entry_uncompressed_bytes: 1_000_000,
                max_compression_ratio: 3,
                max_path_depth: 20,
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("high compression ratio"));
    }

    #[test]
    fn unzip_rejects_total_size_over_limit() {
        let temp = tempdir().unwrap();
        let archive_path = temp.path().join("oversize.zip");
        {
            let file = fs::File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    "a.txt",
                    zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(b"1234567890").unwrap();
            writer
                .start_file(
                    "b.txt",
                    zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(b"abcdefghij").unwrap();
            writer.finish().unwrap();
        }

        let destination = temp.path().join("out");
        let error = unzip_archive_sync_with_limits(
            &archive_path,
            &destination,
            UnzipLimits {
                max_entries: 100,
                max_total_uncompressed_bytes: 15,
                max_entry_uncompressed_bytes: 100,
                max_compression_ratio: 10_000,
                max_path_depth: 20,
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("total extracted size exceeds limit"));
    }
}
