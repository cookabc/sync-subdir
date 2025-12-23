use anyhow::{anyhow, Context, Result};
use std::path::{PathBuf};
use crate::git::{CommitInfo, GitManager};
use tokio::time::{sleep, Duration};
use tempfile::tempdir;

#[derive(Debug, Clone)]
pub struct SyncStats {
    pub total_commits: usize,
    pub synced_commits: usize,
    pub failed_commits: usize,
    pub skipped_commits: usize,
}

impl Default for SyncStats {
    fn default() -> Self {
        Self {
            total_commits: 0,
            synced_commits: 0,
            failed_commits: 0,
            skipped_commits: 0,
        }
    }
}

pub struct SyncEngine {
    config: SyncConfig,
    dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub source_repo: PathBuf,
    pub target_repo: PathBuf,
    pub subdir: String,
}

impl SyncEngine {
    pub fn new(config: SyncConfig, dry_run: bool) -> Self {
        Self {
            config,
            dry_run,
        }
    }

    pub async fn sync_commits<F>(
        &mut self, 
        git_manager: &GitManager,
        commits: &[CommitInfo], 
        mut progress_callback: F
    ) -> Result<SyncStats>
    where
        F: FnMut(usize, usize, &str, &str), // current, total, subject, status
    {
        let mut stats = SyncStats::default();
        stats.total_commits = commits.len();

        if stats.total_commits == 0 {
            return Ok(stats);
        }

        let tmp_dir = tempdir().context("Failed to create temp directory for patches")?;

        for (i, commit) in commits.iter().enumerate() {
            let status = if self.dry_run {
                stats.synced_commits += 1;
                "PREVIEW"
            } else {
                // 1. Create patch
                match git_manager.create_patch_file(&commit.id, &self.config.subdir, tmp_dir.path()) {
                    Ok(patch_path) => {
                        // 2. Apply patch
                        match git_manager.apply_patch_file(&patch_path, None) {
                            Ok(_) => {
                                stats.synced_commits += 1;
                                "OK"
                            }
                            Err(e) if e.to_string() == "EMPTY_PATCH" => {
                                stats.skipped_commits += 1;
                                "EMPTY (SKIPPED)"
                            }
                            Err(e) => {
                                return Err(anyhow!("同步提交失败 {}: {}", commit.id, e));
                            }
                        }
                    }
                    Err(e) => {
                        return Err(anyhow!("生成补丁失败 {}: {}", commit.id, e));
                    }
                }
            };

            progress_callback(i + 1, stats.total_commits, &commit.subject, status);

            // Small delay for UI updates
            sleep(Duration::from_millis(50)).await;
        }

        Ok(stats)
    }

    pub fn validate_paths(&self) -> Result<()> {
        if !self.config.source_repo.exists() {
            return Err(anyhow!("Source repository does not exist: {}", self.config.source_repo.display()));
        }
        if !self.config.target_repo.exists() {
            return Err(anyhow!("Target repository does not exist: {}", self.config.target_repo.display()));
        }
        Ok(())
    }
}