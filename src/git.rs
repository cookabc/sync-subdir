use crate::error::{SyncError, Result};
use tracing::{debug, error};
use git2::{Repository, StatusOptions, Commit, DiffDelta, Signature};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub is_merge: bool,
}

#[derive(Debug)]
pub struct RepoInfo {
    pub path: PathBuf,
    pub current_branch: String,
    pub original_branch: String,
}

pub struct GitManager {
    pub source_repo_info: RepoInfo,
    pub target_repo_info: RepoInfo,
}

/// RAII guard to ensure stash is popped when dropped
pub struct StashGuard<'a> {
    repo: Repository,
    is_active: bool,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> StashGuard<'a> {
    pub fn new(repo: Repository) -> Self {
        Self {
            repo,
            is_active: true,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<'a> Drop for StashGuard<'a> {
    fn drop(&mut self) {
        if self.is_active {
            debug!("StashGuard: Popping stash automatically");
            if let Err(e) = self.repo.stash_pop(0, None) {
                error!("Failed to pop stash in drop: {}", e);
            }
        }
    }
}

/// RAII guard to ensure branch is restored when dropped
pub struct BranchGuard {
    repo_path: PathBuf,
    original_branch: String,
    is_active: bool,
}

impl BranchGuard {
    pub fn new(repo_path: PathBuf, _is_source: bool, original_branch: String) -> Self {
        Self {
            repo_path,
            original_branch,
            is_active: true,
        }
    }
}

impl Drop for BranchGuard {
    fn drop(&mut self) {
        if self.is_active {
            debug!("BranchGuard: Restoring branch {}", self.original_branch);
            if let Ok(repo) = Repository::open(&self.repo_path) {
                let branch_ref = format!("refs/heads/{}", self.original_branch);
                if let Err(e) = repo.set_head(&branch_ref) {
                    error!("Failed to restore branch {} in drop: {}", self.original_branch, e);
                }
            } else {
                error!("Failed to open repository in BranchGuard drop");
            }
        }
    }
}

impl GitManager {
    pub fn new(source_path: &Path, target_path: &Path) -> Result<Self> {
        let source_repo = Repository::open(source_path)
            .map_err(|_| SyncError::NotARepository(source_path.to_path_buf()))?;
        let target_repo = Repository::open(target_path)
            .map_err(|_| SyncError::NotARepository(target_path.to_path_buf()))?;

        let source_current_branch = Self::get_current_branch(&source_repo)?;
        let target_current_branch = Self::get_current_branch(&target_repo)?;

        Ok(Self {
            source_repo_info: RepoInfo {
                path: source_path.to_path_buf(),
                current_branch: source_current_branch.clone(),
                original_branch: source_current_branch,
            },
            target_repo_info: RepoInfo {
                path: target_path.to_path_buf(),
                current_branch: target_current_branch.clone(),
                original_branch: target_current_branch,
            },
        })
    }

    pub fn get_repository(&self, is_source: bool) -> Result<Repository> {
        let path = if is_source {
            &self.source_repo_info.path
        } else {
            &self.target_repo_info.path
        };
        Repository::open(path).map_err(|e| e.into())
    }

    fn get_current_branch(repo: &Repository) -> Result<String> {
        let head = repo.head()?;

        if let Some(name) = head.shorthand() {
            Ok(name.to_string())
        } else {
            // Detached HEAD, get commit hash
            let commit = head.peel_to_commit()?;
            Ok(format!("detached-{}", commit.id()))
        }
    }

    pub fn switch_branch(&mut self, is_source: bool, branch_name: &str) -> Result<()> {
        let repo = self.get_repository(is_source)?;
        let branch_ref = format!("refs/heads/{}", branch_name);

        // Check if branch exists
        let _branch_oid = repo.revparse_single(&branch_ref)
            .map_err(|_| SyncError::BranchNotFound(branch_name.to_string()))?
            .id();

        // Checkout the branch
        repo.set_head(&branch_ref)?;

        // Update current branch info
        if is_source {
            self.source_repo_info.current_branch = branch_name.to_string();
        } else {
            self.target_repo_info.current_branch = branch_name.to_string();
        }

        Ok(())
    }

    pub fn create_branch(&mut self, is_target: bool, branch_name: &str) -> Result<()> {
        let repo = self.get_repository(is_target)?;
        let head = repo.head()?;
        let head_commit = head.peel_to_commit()?;

        let _branch = repo.branch(branch_name, &head_commit, false)?;

        // Checkout the new branch
        repo.set_head(&format!("refs/heads/{}", branch_name))?;

        if is_target {
            self.target_repo_info.current_branch = branch_name.to_string();
        }

        Ok(())
    }

    pub fn has_uncommitted_changes(&self, is_target: bool) -> Result<bool> {
        let repo = self.get_repository(is_target)?;
        let mut status_options = StatusOptions::new();
        status_options.include_untracked(true);

        let statuses = repo.statuses(Some(&mut status_options))?;

        Ok(!statuses.is_empty())
    }

    pub fn stash_changes(&self, is_target: bool, message: &str) -> Result<()> {
        let mut repo = self.get_repository(is_target)?;

        // Get current signature
        let signature = repo.signature()
            .unwrap_or_else(|_| Signature::now("sync-subdir", "sync-subdir@example.com").unwrap());

        // Stash changes
        match repo.stash_save(&signature, message, None) {
            Ok(_) => Ok(()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                debug!("Nothing to stash in {} repo", if is_target { "target" } else { "source" });
                Ok(())
            }
            Err(e) => Err(SyncError::Git(e)),
        }
    }


    pub fn validate_commit(&self, is_source: bool, commit_hash: &str) -> Result<()> {
        let repo = self.get_repository(is_source)?;
        repo.revparse_single(commit_hash)
            .map_err(|_| SyncError::InvalidCommit(commit_hash.to_string()))?;
        Ok(())
    }

    pub fn get_commits_in_range(
        &self,
        subdir: &str,
        start_commit: &str,
        end_commit: &str,
        include_start: bool,
        first_parent: bool,
    ) -> Result<Vec<CommitInfo>> {
        debug!("get_commits_in_range: subdir={}, start={}, end={}, include_start={}, first_parent={}", 
               subdir, start_commit, end_commit, include_start, first_parent);
        let repo = self.get_repository(true)?;

        let start_obj = repo.revparse_single(start_commit)
            .map_err(|_| SyncError::InvalidCommit(start_commit.to_string()))?;
        let end_obj = repo.revparse_single(end_commit)
            .map_err(|_| SyncError::InvalidCommit(end_commit.to_string()))?;

        let start_oid = start_obj.id();
        let end_oid = end_obj.id();

        let start_commit_obj = start_obj.peel_to_commit()?;
        
        // Determine the commit range starting point
        let range_start = if include_start {
            if let Ok(parent) = start_commit_obj.parent(0) {
                parent.id()
            } else {
                start_oid // Root commit
            }
        } else {
            start_oid
        };

        let mut revwalk = repo.revwalk()?;
        revwalk.push_range(&format!("{}..{}", range_start, end_oid))?;
        if first_parent {
            revwalk.simplify_first_parent()?;
        }
        revwalk.set_sorting(git2::Sort::REVERSE | git2::Sort::TIME)?;

        let mut commit_infos = Vec::new();

        for id in revwalk {
            let id = id?;
            let commit = repo.find_commit(id)?;
            
            // Check if commit affects the subdirectory
            let affects = if subdir.is_empty() || subdir == "." {
                true
            } else {
                self.commit_affects_subdir(&commit, subdir)?
            };

            if affects {
                commit_infos.push(CommitInfo {
                    id: id.to_string(),
                    subject: commit.summary().unwrap_or("No subject").to_string(),
                    author: commit.author().name().unwrap_or("Unknown").to_string(),
                    date: chrono::DateTime::<chrono::Utc>::from_timestamp(commit.time().seconds(), 0)
                        .unwrap_or_default()
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string(),
                    is_merge: commit.parents().len() > 1,
                });
            }
        }

        Ok(commit_infos)
    }

    pub fn create_patch_file(&self, commit_id: &str, subdir: &str, output_dir: &Path) -> Result<PathBuf> {
        let repo_path = &self.source_repo_info.path;
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .arg("format-patch")
            .arg("-1")
            .arg(commit_id)
            .arg("--binary")
            .arg("--full-index")
            .arg(format!("--relative={}", subdir))
            .arg("-o")
            .arg(output_dir)
            .output()?;

        if !output.status.success() {
            return Err(SyncError::PatchGenerationFailed(String::from_utf8_lossy(&output.stderr).to_string()));
        }

        let patch_file_name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if patch_file_name.is_empty() {
             // Sometimes format-patch outputs nothing to stdout if -o is used, 
             // we need to find the file in output_dir
             let entries = std::fs::read_dir(output_dir)?;
             for entry in entries {
                 let entry = entry?;
                 return Ok(entry.path());
             }
             return Err(SyncError::PatchGenerationFailed("No patch file generated".to_string()));
        }
        
        Ok(output_dir.join(patch_file_name))
    }

    pub fn apply_patch_file(&self, patch_path: &Path, target_subdir: Option<&str>) -> Result<()> {
        let repo_path = &self.target_repo_info.path;
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(repo_path).arg("am");
        
        cmd.arg("--3way").arg("--committer-date-is-author-date");
        
        if let Some(subdir) = target_subdir {
            cmd.arg(format!("--directory={}", subdir));
        }
        
        cmd.arg(patch_path);

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("patch does not have a valid index") || stderr.contains("Patch is empty") {
                return Err(SyncError::EmptyPatch);
            }
            return Err(SyncError::PatchConflict(stderr.to_string()));
        }

        Ok(())
    }


    #[allow(dead_code)]
    pub fn get_commit_count(&self, subdir: &str, start_commit: &str, end_commit: &str, _exclude_merges: bool) -> Result<(usize, usize)> {
        let repo = self.get_repository(true)?;

        // Resolve commit references (supports both OIDs and references like HEAD)
        let start_obj = repo.revparse_single(start_commit)
            .map_err(|_| SyncError::InvalidCommit(start_commit.to_string()))?;
        let end_obj = repo.revparse_single(end_commit)
            .map_err(|_| SyncError::InvalidCommit(end_commit.to_string()))?;

        let _start_oid = start_obj.id();
        let _end_oid = end_obj.id();

        let mut revwalk = repo.revwalk()?;
        revwalk.push_range(&format!("{}..{}", start_commit, end_commit))?;

        let mut total_commits = 0;
        let mut merge_commits = 0;

        for id in revwalk {
            let id = id?;
            let commit = repo.find_commit(id)?;

            // Check if commit affects the subdirectory
            let affects_subdir = self.commit_affects_subdir(&commit, subdir)?;
            if !affects_subdir {
                continue;
            }

            total_commits += 1;
            if commit.parents().len() > 1 {
                merge_commits += 1;
            }
        }

        Ok((total_commits, merge_commits))
    }

    #[allow(dead_code)]
    fn commit_affects_subdir(&self, commit: &Commit, subdir: &str) -> Result<bool> {
        let repo = self.get_repository(true)?;

        if let Ok(parent) = commit.parent(0) {
            let tree_a = parent.tree()?;
            let tree_b = commit.tree()?;

            let diff = repo.diff_tree_to_tree(Some(&tree_a), Some(&tree_b), None)?;
            let subdir_pattern = format!("{}/", subdir.trim_end_matches('/'));

            let mut affects_subdir = false;
            let result = diff.foreach(
                &mut |delta: DiffDelta, _progress| {
                    let new_path = delta.new_file().path()
                        .and_then(|p| p.to_str())
                        .unwrap_or("");

                    let old_path = delta.old_file().path()
                        .and_then(|p| p.to_str())
                        .unwrap_or("");

                    if new_path.starts_with(&subdir_pattern) || old_path.starts_with(&subdir_pattern) {
                        affects_subdir = true;
                        return false; // Stop iteration
                    }
                    true
                },
                None,
                None,
                None,
            );

            match result {
                Ok(_) => Ok(affects_subdir),
                Err(e) if e.code() == git2::ErrorCode::User => Ok(affects_subdir),
                Err(e) => Err(e.into()),
            }
        } else {
            // Initial commit, check if it contains files in the subdirectory
            let tree_b = commit.tree()?;
            let diff = repo.diff_tree_to_tree(None, Some(&tree_b), None)?;
            let subdir_pattern = format!("{}/", subdir.trim_end_matches('/'));

            let mut affects_subdir = false;
            let result = diff.foreach(
                &mut |delta: DiffDelta, _progress| {
                    let new_path = delta.new_file().path()
                        .and_then(|p| p.to_str())
                        .unwrap_or("");

                    if new_path.starts_with(&subdir_pattern) || new_path == subdir {
                        affects_subdir = true;
                        return false; // Stop iteration
                    }
                    true
                },
                None,
                None,
                None,
            );

            match result {
                Ok(_) => Ok(affects_subdir),
                Err(e) if e.code() == git2::ErrorCode::User => Ok(affects_subdir),
                Err(e) => Err(e.into()),
            }
        }
    }
}