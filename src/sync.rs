use crate::error::{SyncError, Result};
use crate::git::{CommitInfo, GitManager};
use tokio::time::{sleep, Duration};
use tokio::sync::mpsc::UnboundedSender;
use tempfile::tempdir;

#[derive(Debug, Clone)]
pub enum SyncEvent {
    Progress {
        current: usize,
        total: usize,
        subject: String,
        status: String,
    },
    Completed(SyncStats),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct SyncStats {
    pub total_commits: usize,
    pub synced_commits: usize,
    pub skipped_commits: usize,
}

impl Default for SyncStats {
    fn default() -> Self {
        Self {
            total_commits: 0,
            synced_commits: 0,
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
    pub subdir: String,
}

impl SyncEngine {
    pub fn new(config: SyncConfig, dry_run: bool) -> Self {
        Self {
            config,
            dry_run,
        }
    }

    pub async fn sync_commits(
        &mut self, 
        git_manager: &GitManager,
        commits: &[CommitInfo], 
        tx: UnboundedSender<SyncEvent>,
    ) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        stats.total_commits = commits.len();

        if stats.total_commits == 0 {
            let _ = tx.send(SyncEvent::Completed(stats.clone()));
            return Ok(stats);
        }

        let tmp_dir = tempdir().map_err(|e| SyncError::Io(e))?;

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
                            Err(SyncError::EmptyPatch) => {
                                stats.skipped_commits += 1;
                                "EMPTY (SKIPPED)"
                            }
                            Err(e) => {
                                let err_msg = format!("同步提交失败 {}: {}", commit.id, e);
                                let _ = tx.send(SyncEvent::Error(err_msg));
                                return Err(e);
                            }
                        }
                    }
                    Err(e) => {
                        let err_msg = format!("生成补丁失败 {}: {}", commit.id, e);
                        let _ = tx.send(SyncEvent::Error(err_msg));
                        return Err(e);
                    }
                }
            };

            let _ = tx.send(SyncEvent::Progress {
                current: i + 1,
                total: stats.total_commits,
                subject: commit.subject.clone(),
                status: status.to_string(),
            });

            // Small delay for UI updates (reduced from 50ms to 20ms for better responsiveness)
            sleep(Duration::from_millis(20)).await;
        }

        let _ = tx.send(SyncEvent::Completed(stats.clone()));
        Ok(stats)
    }
}