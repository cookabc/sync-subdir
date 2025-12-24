use thiserror::Error;
use std::path::PathBuf;

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Anyhow error: {0}")]
    Anyhow(#[from] anyhow::Error),

    #[error("Invalid commit hash: {0}")]
    InvalidCommit(String),

    #[error("Path does not exist: {0}")]
    PathNotFound(PathBuf),

    #[error("Not a git repository: {0}")]
    NotARepository(PathBuf),

    #[error("Repository has uncommitted changes: {0}")]
    DirtyRepository(PathBuf),

    #[error("Empty patch: the commit does not affect the specified subdirectory")]
    EmptyPatch,

    #[error("Patch conflict: {0}")]
    PatchConflict(String),

    #[error("Branch not found: {0}")]
    BranchNotFound(String),

    #[error("Failed to generate patch: {0}")]
    PatchGenerationFailed(String),
}

pub type Result<T> = std::result::Result<T, SyncError>;
