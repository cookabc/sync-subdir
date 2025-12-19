use anyhow::{Context, Result};
use tracing::{error, debug};
use git2::{Repository, StatusOptions, Oid, Commit, Diff, DiffDelta};
use std::path::{Path, PathBuf};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: String,
    pub status: FileStatus,
    pub old_path: Option<String>, // For renamed files
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

impl GitManager {
    pub fn new(source_path: &Path, target_path: &Path) -> Result<Self> {
        let source_repo = Repository::open(source_path)
            .with_context(|| format!("Failed to open source repository: {}", source_path.display()))?;
        let target_repo = Repository::open(target_path)
            .with_context(|| format!("Failed to open target repository: {}", target_path.display()))?;

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
        Repository::open(path).with_context(|| format!("Failed to open repository: {}", path.display()))
    }

    fn get_current_branch(repo: &Repository) -> Result<String> {
        let head = repo.head()
            .with_context(|| "Failed to get repository HEAD")?;

        if let Some(name) = head.shorthand() {
            Ok(name.to_string())
        } else {
            // Detached HEAD, get commit hash
            let commit = head.peel_to_commit()
                .with_context(|| "Failed to peel HEAD to commit")?;
            Ok(format!("detached-{}", commit.id()))
        }
    }

    pub fn switch_branch(&mut self, is_source: bool, branch_name: &str) -> Result<()> {
        let repo = self.get_repository(is_source)?;
        let branch_ref = format!("refs/heads/{}", branch_name);

        // Check if branch exists
        let _branch_oid = repo.revparse_single(&branch_ref)
            .with_context(|| format!("Branch '{}' does not exist", branch_name))?
            .id();

        // Checkout the branch
        repo.set_head(&branch_ref)
            .with_context(|| format!("Failed to set HEAD to branch: {}", branch_name))?;

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
        let head = repo.head()
            .with_context(|| "Failed to get repository HEAD")?;
        let head_commit = head.peel_to_commit()
            .with_context(|| "Failed to peel HEAD to commit")?;

        let _branch = repo.branch(branch_name, &head_commit, false)
            .with_context(|| format!("Failed to create branch: {}", branch_name))?;

        // Checkout the new branch
        repo.set_head(&format!("refs/heads/{}", branch_name))
            .with_context(|| format!("Failed to set HEAD to new branch: {}", branch_name))?;

        if is_target {
            self.target_repo_info.current_branch = branch_name.to_string();
        }

        Ok(())
    }

    pub fn has_uncommitted_changes(&self, is_target: bool) -> Result<bool> {
        let repo = self.get_repository(is_target)?;
        let mut status_options = StatusOptions::new();
        status_options.include_untracked(true);

        let statuses = repo.statuses(Some(&mut status_options))
            .with_context(|| "Failed to get repository status")?;

        Ok(!statuses.is_empty())
    }

    pub fn stash_changes(&self, is_target: bool, message: &str) -> Result<()> {
        let mut repo = self.get_repository(is_target)?;

        // Get current signature
        let signature = repo.signature()
            .or_else(|_| git2::Signature::now("sync-subdir", "sync-subdir@example.com"))
            .with_context(|| "Failed to create git signature")?;

        // Stash changes
        repo.stash_save(&signature, message, None)
            .with_context(|| "Failed to stash changes")?;

        Ok(())
    }

    pub fn pop_stash(&self, is_target: bool) -> Result<()> {
        let mut repo = self.get_repository(is_target)?;
        repo.stash_pop(0, None)
            .with_context(|| "Failed to pop stash")?;
        Ok(())
    }

    pub fn validate_commit(&self, is_source: bool, commit_hash: &str) -> Result<()> {
        let repo = self.get_repository(is_source)?;
        repo.revparse_single(commit_hash)
            .with_context(|| format!("Invalid commit hash: {}", commit_hash))?;
        Ok(())
    }

    pub fn get_file_changes(
        &self,
        subdir: &str,
        start_commit: &str,
        end_commit: &str,
        include_start: bool,
        exclude_merges: bool,
    ) -> Result<Vec<FileChange>> {
        debug!("get_file_changes: subdir={}, start={}, end={}, include_start={}, exclude_merges={}", 
               subdir, start_commit, end_commit, include_start, exclude_merges);
        let repo = self.get_repository(true)?;

        // Resolve commit references (supports both OIDs and references like HEAD)
        let start_obj = repo.revparse_single(start_commit)
            .with_context(|| format!("Invalid start commit: {}", start_commit))?;
        let end_obj = repo.revparse_single(end_commit)
            .with_context(|| format!("Invalid end commit: {}", end_commit))?;

        let start_oid = start_obj.id();
        let end_oid = end_obj.id();

        let start_commit_obj = start_obj.peel_to_commit()
            .with_context(|| format!("Start commit not found: {}", start_commit))?;
        debug!("Resolved start commit to: {}", start_commit_obj.id());
        
        let _end_commit_obj = end_obj.peel_to_commit()
            .with_context(|| format!("End commit not found: {}", end_commit))?;
        debug!("Resolved end commit to: {}", _end_commit_obj.id());

        let mut changes = Vec::new();
        let subdir_pattern = format!("{}/", subdir.trim_end_matches('/'));

        // Determine the commit range
        let (range_start, include_start_changes) = if include_start {
            if let Ok(parent) = start_commit_obj.parent(0) {
                (parent.id(), true)
            } else {
                // Start commit is the root commit
                (start_oid, false)
            }
        } else {
            (start_oid, false)
        };

        // Get commit range
        let mut revwalk = repo.revwalk()
            .with_context(|| "Failed to create revwalk")?;
        revwalk.push_range(&format!("{}..{}", range_start, end_oid))
            .with_context(|| "Failed to set commit range")?;

        // Collect commits (reverse to get chronological order)
        let mut commits: Vec<Commit> = revwalk
            .filter_map(|id| {
                match id {
                    Ok(id) => repo.find_commit(id).ok(),
                    Err(e) => {
                        error!("Revwalk error: {}", e);
                        None
                    }
                }
            })
            .collect();
        debug!("Found {} total commits in range", commits.len());
        commits.reverse();
        debug!("Processing commits in chronological order");

        // Process each commit
        for commit in commits {
            // Skip merge commits if requested
            if exclude_merges && commit.parents().len() > 1 {
                continue;
            }

            // Get changes for this commit
            if let Ok(parent) = commit.parent(0) {
                let tree_a = parent.tree()?;
                let tree_b = commit.tree()?;

                let mut diff = repo.diff_tree_to_tree(Some(&tree_a), Some(&tree_b), None)?;
                self.process_diff(&mut diff, &subdir_pattern, &mut changes)?;
            } else if commit.parents().len() == 0 {
                // Initial commit - compare to empty tree
                let tree_b = commit.tree()?;
                let empty_tree_id = Oid::zero();
                let empty_tree = repo.find_tree(empty_tree_id).ok();

                let mut diff = repo.diff_tree_to_tree(empty_tree.as_ref(), Some(&tree_b), None)?;
                self.process_diff(&mut diff, &subdir_pattern, &mut changes)?;
            }
        }

        // Handle root commit inclusion
        if include_start_changes && start_commit_obj.parents().len() == 0 {
            let tree_b = start_commit_obj.tree()?;
            let empty_tree_id = Oid::zero();
            let empty_tree = repo.find_tree(empty_tree_id).ok();

            let mut diff = repo.diff_tree_to_tree(empty_tree.as_ref(), Some(&tree_b), None)?;
            self.process_diff(&mut diff, &subdir_pattern, &mut changes)?;
        }

        // Remove duplicates and sort
        let mut seen_paths = HashSet::new();
        changes.retain(|change| {
            if seen_paths.contains(&change.path) {
                false
            } else {
                seen_paths.insert(change.path.clone());
                true
            }
        });

        Ok(changes)
    }

    fn process_diff(&self, diff: &mut Diff, subdir_pattern: &str, changes: &mut Vec<FileChange>) -> Result<()> {
        diff.foreach(
            &mut |delta: DiffDelta, _progress| {
                let new_path = delta.new_file().path()
                    .and_then(|p| p.to_str())
                    .unwrap_or("");

                let old_path = delta.old_file().path()
                    .and_then(|p| p.to_str())
                    .unwrap_or("");

                // Check if either path is in the subdirectory
                let in_subdir = new_path.starts_with(subdir_pattern) || old_path.starts_with(subdir_pattern);

                if in_subdir {
                    let (path, status) = match delta.status() {
                        git2::Delta::Added => {
                            (new_path.to_string(), FileStatus::Added)
                        }
                        git2::Delta::Deleted => {
                            (old_path.to_string(), FileStatus::Deleted)
                        }
                        git2::Delta::Modified => {
                            (new_path.to_string(), FileStatus::Modified)
                        }
                        git2::Delta::Renamed => {
                            let change = FileChange {
                                path: new_path.to_string(),
                                status: FileStatus::Renamed,
                                old_path: Some(old_path.to_string()),
                            };
                            changes.push(change);
                            return true;
                        }
                        git2::Delta::Typechange => {
                            (new_path.to_string(), FileStatus::TypeChanged)
                        }
                        _ => return true, // Ignore other types
                    };

                    changes.push(FileChange {
                        path,
                        status,
                        old_path: None,
                    });
                }
                true
            },
            None,
            None,
            None,
        )?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_commit_count(&self, subdir: &str, start_commit: &str, end_commit: &str, _exclude_merges: bool) -> Result<(usize, usize)> {
        let repo = self.get_repository(true)?;

        // Resolve commit references (supports both OIDs and references like HEAD)
        let start_obj = repo.revparse_single(start_commit)
            .with_context(|| format!("Invalid start commit: {}", start_commit))?;
        let end_obj = repo.revparse_single(end_commit)
            .with_context(|| format!("Invalid end commit: {}", end_commit))?;

        let _start_oid = start_obj.id();
        let _end_oid = end_obj.id();

        let mut revwalk = repo.revwalk()
            .with_context(|| "Failed to create revwalk")?;
        revwalk.push_range(&format!("{}..{}", start_commit, end_commit))
            .with_context(|| "Failed to set commit range")?;

        let mut total_commits = 0;
        let mut merge_commits = 0;

        for id in revwalk {
            let id = id.with_context(|| "Failed to get commit ID from revwalk")?;
            let commit = repo.find_commit(id)
                .with_context(|| "Failed to find commit")?;

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
            diff.foreach(
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
            )?;

            Ok(affects_subdir)
        } else {
            // Initial commit, check if it contains files in the subdirectory
            let tree_b = commit.tree()?;
            let empty_tree_id = Oid::zero();
            let empty_tree = repo.find_tree(empty_tree_id).ok();

            let diff = repo.diff_tree_to_tree(empty_tree.as_ref(), Some(&tree_b), None)?;
            let subdir_pattern = format!("{}/", subdir.trim_end_matches('/'));

            let mut affects_subdir = false;
            diff.foreach(
                &mut |delta: DiffDelta, _progress| {
                    let new_path = delta.new_file().path()
                        .and_then(|p| p.to_str())
                        .unwrap_or("");

                    if new_path.starts_with(&subdir_pattern) {
                        affects_subdir = true;
                        return false; // Stop iteration
                    }
                    true
                },
                None,
                None,
                None,
            )?;

            Ok(affects_subdir)
        }
    }

    pub fn restore_original_branches(&mut self) -> Result<()> {
        // Store the original branch names to avoid borrowing issues
        let source_original = self.source_repo_info.original_branch.clone();
        let target_original = self.target_repo_info.original_branch.clone();

        // Restore source repository
        if self.source_repo_info.current_branch != source_original {
            self.switch_branch(true, &source_original)?;
        }

        // Restore target repository
        if self.target_repo_info.current_branch != target_original {
            self.switch_branch(false, &target_original)?;
        }

        Ok(())
    }
}