use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::fs;
use fs_extra::file::{copy, CopyOptions};
use crate::git::{FileChange, FileStatus};
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone)]
pub struct SyncStats {
    pub total_files: usize,
    pub synced_files: usize,
    pub deleted_files: usize,
    pub failed_files: usize,
    pub skipped_files: usize,
}

impl Default for SyncStats {
    fn default() -> Self {
        Self {
            total_files: 0,
            synced_files: 0,
            deleted_files: 0,
            failed_files: 0,
            skipped_files: 0,
        }
    }
}

pub struct SyncEngine {
    config: SyncConfig,
    dry_run: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SyncConfig {
    pub source_repo: PathBuf,
    pub target_repo: PathBuf,
    pub subdir: String,
    pub sync_delete: bool,
    pub verbose: bool,
}

impl SyncEngine {
    pub fn new(config: SyncConfig, dry_run: bool) -> Self {
        Self {
            config,
            dry_run,
        }
    }

    pub async fn sync_files<F>(&mut self, file_changes: &[FileChange], selected_files: &[bool], mut progress_callback: F) -> Result<SyncStats>
    where
        F: FnMut(usize, usize, &str, &str), // current, total, file_path, status
    {
        let mut stats = SyncStats::default();
        stats.total_files = selected_files.iter().filter(|&&selected| selected).count();

        if stats.total_files == 0 {
            return Ok(stats);
        }

        for (i, (change, &selected)) in file_changes.iter().zip(selected_files.iter()).enumerate() {
            if !selected {
                continue;
            }

            let relative_path = self.strip_subdir_prefix(&change.path)?;
            let source_path = self.config.source_repo.join(&change.path);
            let target_path = self.config.target_repo.join(&relative_path);

            let status = match self.sync_single_file(change, &source_path, &target_path).await {
                Ok(sync_status) => {
                    match sync_status {
                        SyncStatus::Synced => {
                            stats.synced_files += 1;
                            "已同步"
                        }
                        SyncStatus::Deleted => {
                            stats.deleted_files += 1;
                            "已删除"
                        }
                        SyncStatus::Skipped => {
                            stats.skipped_files += 1;
                            "已跳过"
                        }
                    }
                }
                Err(e) => {
                    stats.failed_files += 1;
                    eprintln!("同步文件失败 {}: {}", change.path, e);
                    "失败"
                }
            };

            progress_callback(i + 1, stats.total_files, &relative_path, status);

            // Small delay to prevent overwhelming the filesystem
            if !self.dry_run {
                sleep(Duration::from_millis(10)).await;
            }
        }

        Ok(stats)
    }

    async fn sync_single_file(&self, change: &FileChange, source_path: &Path, target_path: &Path) -> Result<SyncStatus> {
        match change.status {
            FileStatus::Added | FileStatus::Modified | FileStatus::TypeChanged => {
                if !source_path.exists() {
                    return Ok(SyncStatus::Skipped);
                }

                if self.dry_run {
                    return Ok(SyncStatus::Synced);
                }

                // Create parent directories if they don't exist
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
                }

                // Copy the file
                let mut options = CopyOptions::new();
                options.overwrite = true;
                copy(source_path, target_path, &options)
                    .with_context(|| format!(
                        "Failed to copy {} to {}",
                        source_path.display(),
                        target_path.display()
                    ))?;

                Ok(SyncStatus::Synced)
            }
            FileStatus::Deleted => {
                if !self.config.sync_delete {
                    return Ok(SyncStatus::Skipped);
                }

                if self.dry_run {
                    return Ok(SyncStatus::Deleted);
                }

                if target_path.exists() {
                    fs::remove_file(target_path)
                        .with_context(|| format!("Failed to delete file: {}", target_path.display()))?;

                    // Try to remove empty parent directories (optional cleanup)
                    self.try_cleanup_empty_dirs(target_path)?;
                }

                Ok(SyncStatus::Deleted)
            }
            FileStatus::Renamed => {
                // Handle rename as delete + add
                let old_relative = if let Some(ref old_path) = change.old_path {
                    self.strip_subdir_prefix(old_path)?
                } else {
                    return Ok(SyncStatus::Skipped);
                };

                let old_target_path = self.config.target_repo.join(&old_relative);

                if self.dry_run {
                    return Ok(SyncStatus::Synced);
                }

                // Delete old file
                if old_target_path.exists() {
                    fs::remove_file(&old_target_path)
                        .with_context(|| format!("Failed to delete renamed file: {}", old_target_path.display()))?;
                }

                // Copy new file
                if source_path.exists() {
                    if let Some(parent) = target_path.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
                    }

                    let mut options = CopyOptions::new();
                    options.overwrite = true;
                    copy(source_path, target_path, &options)
                        .with_context(|| format!(
                            "Failed to copy renamed file {} to {}",
                            source_path.display(),
                            target_path.display()
                        ))?;
                }

                Ok(SyncStatus::Synced)
            }
        }
    }

    fn try_cleanup_empty_dirs(&self, path: &Path) -> Result<()> {
        // Try to remove empty parent directories up to the target repo root
        let mut current_path = path.parent();

        while let Some(parent) = current_path {
            // Don't delete beyond the target repository
            if !parent.starts_with(&self.config.target_repo) {
                break;
            }

            // Only remove if directory is empty
            if self.is_dir_empty(parent)? {
                fs::remove_dir(parent)
                    .with_context(|| format!("Failed to remove empty directory: {}", parent.display()))?;
            } else {
                break; // Directory not empty, stop cleanup
            }

            current_path = parent.parent();
        }

        Ok(())
    }

    fn is_dir_empty(&self, path: &Path) -> Result<bool> {
        let mut entries = fs::read_dir(path)
            .with_context(|| format!("Failed to read directory: {}", path.display()))?;

        Ok(entries.next().is_none())
    }

    fn strip_subdir_prefix(&self, path: &str) -> Result<String> {
        let subdir_pattern = format!("{}/", self.config.subdir.trim_end_matches('/'));

        if path.starts_with(&subdir_pattern) {
            Ok(path[subdir_pattern.len()..].to_string())
        } else {
            Err(anyhow!("Path {} does not start with subdirectory {}", path, self.config.subdir))
        }
    }

    pub fn validate_paths(&self) -> Result<()> {
        // Check if source repository exists
        if !self.config.source_repo.exists() {
            return Err(anyhow!("Source repository does not exist: {}", self.config.source_repo.display()));
        }

        // Check if target repository exists
        if !self.config.target_repo.exists() {
            return Err(anyhow!("Target repository does not exist: {}", self.config.target_repo.display()));
        }

        // Check if subdirectory exists in source
        let source_subdir = self.config.source_repo.join(&self.config.subdir);
        if !source_subdir.exists() {
            return Err(anyhow!("Subdirectory does not exist in source repository: {}", source_subdir.display()));
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_deletion_count(&self, file_changes: &[FileChange], selected_files: &[bool]) -> usize {
        file_changes
            .iter()
            .zip(selected_files.iter())
            .filter(|(change, &selected)| {
                selected && matches!(change.status, FileStatus::Deleted)
            })
            .count()
    }

    #[allow(dead_code)]
    pub async fn preview_sync(&self, file_changes: &[FileChange], selected_files: &[bool]) -> Result<Vec<PreviewItem>> {
        let mut preview_items = Vec::new();

        for (change, &selected) in file_changes.iter().zip(selected_files.iter()) {
            if !selected {
                continue;
            }

            let relative_path = self.strip_subdir_prefix(&change.path)?;
            let source_path = self.config.source_repo.join(&change.path);
            let target_path = self.config.target_repo.join(&relative_path);

            let action = match change.status {
                FileStatus::Added => PreviewAction::Add,
                FileStatus::Modified => PreviewAction::Modify,
                FileStatus::Deleted => PreviewAction::Delete,
                FileStatus::Renamed => PreviewAction::Rename {
                    old_path: self.strip_subdir_prefix(change.old_path.as_ref().unwrap_or(&change.path))?,
                    new_path: relative_path,
                },
                FileStatus::TypeChanged => PreviewAction::Modify,
            };

            preview_items.push(PreviewItem {
                action,
                source_path,
                target_path,
            });
        }

        Ok(preview_items)
    }
}

#[derive(Debug, Clone)]
pub enum SyncStatus {
    Synced,
    Deleted,
    Skipped,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum PreviewAction {
    Add,
    Modify,
    Delete,
    Rename { old_path: String, new_path: String },
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PreviewItem {
    pub action: PreviewAction,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
}

impl PreviewItem {
    #[allow(dead_code)]
    pub fn description(&self) -> String {
        match &self.action {
            PreviewAction::Add => format!("新增: {}", self.target_path.display()),
            PreviewAction::Modify => format!("修改: {}", self.target_path.display()),
            PreviewAction::Delete => format!("删除: {}", self.target_path.display()),
            PreviewAction::Rename { old_path, new_path } => {
                format!("重命名: {} → {}", old_path, new_path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs::File;
    use std::io::Write;

    #[tokio::test]
    async fn test_strip_subdir_prefix() {
        let config = SyncConfig {
            source_repo: PathBuf::from("/source"),
            target_repo: PathBuf::from("/target"),
            subdir: "src".to_string(),
            sync_delete: true,
            verbose: false,
        };

        let engine = SyncEngine::new(config, false);

        // Test normal case
        assert_eq!(engine.strip_subdir_prefix("src/main.rs").unwrap(), "main.rs");
        assert_eq!(engine.strip_subdir_prefix("src/utils/helper.rs").unwrap(), "utils/helper.rs");

        // Test edge case with trailing slash
        assert_eq!(engine.strip_subdir_prefix("src/main.rs").unwrap(), "main.rs");

        // Test error case
        assert!(engine.strip_subdir_prefix("other/file.rs").is_err());
        assert!(engine.strip_subdir_prefix("srcfile.rs").is_err());
    }

    #[tokio::test]
    async fn test_is_dir_empty() {
        let temp_dir = TempDir::new().unwrap();
        let config = SyncConfig {
            source_repo: PathBuf::from("/source"),
            target_repo: PathBuf::from("/target"),
            subdir: "src".to_string(),
            sync_delete: true,
            verbose: false,
        };

        let engine = SyncEngine::new(config, false);

        // Test empty directory
        let empty_dir = temp_dir.path().join("empty");
        fs::create_dir_all(&empty_dir).unwrap();
        assert!(engine.is_dir_empty(&empty_dir).unwrap());

        // Test directory with file
        let nonempty_dir = temp_dir.path().join("nonempty");
        fs::create_dir_all(&nonempty_dir).unwrap();
        let file_path = nonempty_dir.join("test.txt");
        let mut file = File::create(file_path).unwrap();
        file.write_all(b"test").unwrap();
        assert!(!engine.is_dir_empty(&nonempty_dir).unwrap());
    }
}