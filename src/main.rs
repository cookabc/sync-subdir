mod cli;
mod git;
mod tui;
mod sync;
mod error;

use crate::error::{SyncError, Result};
use crate::sync::SyncEvent;
use crossterm::event::{self, Event, KeyCode};
use tracing::{info, Level};
use tracing_subscriber;
use tokio::sync::mpsc;
use std::time::Duration;

use cli::{build_cli, Config};
use git::{GitManager, StashGuard, BranchGuard};
use sync::{SyncEngine, SyncConfig};
use tui::{App, TuiManager, AppState, ConfirmationAction};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    info!("Starting sync-subdir");

    // Parse command line arguments
    let matches = build_cli().get_matches();
    let config = Config::from_matches(matches).map_err(SyncError::Anyhow)?;

    // Validate configuration
    validate_config(&config)?;

    // Initialize Git manager
    let mut git_manager = GitManager::new(&config.source_repo, &config.target_repo)?;

    // Validate commits
    git_manager.validate_commit(true, &config.start_commit)?;
    if let Some(ref end_commit) = config.end_commit {
        git_manager.validate_commit(true, end_commit)?;
    }

    // RAII guards for branch restoration
    let source_original = git_manager.source_repo_info.original_branch.clone();
    let target_original = git_manager.target_repo_info.original_branch.clone();
    
    // Switch branches if specified
    if let Some(ref source_branch) = config.source_branch {
        git_manager.switch_branch(true, source_branch)?;
    }

    // Create a guard for source branch
    let mut _source_guard = BranchGuard::new(config.source_repo.clone(), true, source_original);

    let target_branch = config.get_default_target_branch();

    // Handle target branch creation/switching
    let target_repo = git_manager.get_repository(false)?;
    if !target_repo.revparse_single(&format!("refs/heads/{}", target_branch)).is_ok() {
        if config.create_branch.unwrap_or(true) {
            git_manager.create_branch(false, &target_branch)?;
        } else {
            return Err(SyncError::BranchNotFound(target_branch));
        }
    } else {
        git_manager.switch_branch(false, &target_branch)?;
    }

    // Create a guard for target branch
    let mut _target_guard = BranchGuard::new(config.target_repo.clone(), false, target_original);

    // Handle uncommitted changes in target repo
    let mut _stash_guard = None;
    if git_manager.has_uncommitted_changes(false)? {
        if config.auto_stash.unwrap_or(true) {
            let stash_message = format!("sync-subdir auto stash {}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
            git_manager.stash_changes(false, &stash_message)?;
            _stash_guard = Some(StashGuard::new(git_manager.get_repository(false)?));
        } else {
            return Err(SyncError::DirtyRepository(config.target_repo.clone()));
        }
    }

    // Initialize TUI
    let mut tui_manager = TuiManager::new()
        .map_err(SyncError::Anyhow)?;

    let mut app = App::new(config.clone());

    // Run the application
    run_application(&mut app, &mut tui_manager, &mut git_manager).await?;

    Ok(())
}

async fn run_application(
    app: &mut App,
    tui_manager: &mut TuiManager,
    git_manager: &mut GitManager,
) -> Result<()> {
    let (sync_tx, mut sync_rx) = mpsc::unbounded_channel::<SyncEvent>();
    
    loop {
        tui_manager.draw(app).map_err(SyncError::Anyhow)?;

        // Handle events (Non-blocking selection between TUI keys and Sync events)
        tokio::select! {
            // TUI Events
            Ok(has_event) = tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(50))) => {
                if let Ok(true) = has_event {
                    if let Ok(Event::Key(key_event)) = event::read() {
                        handle_key_event(app, tui_manager, git_manager, key_event.code, &sync_tx).await?;
                    }
                }
            }
            
            // Sync Events from background task
            Some(event) = sync_rx.recv() => {
                handle_sync_event(app, event);
            }

            // Redraw/Idle
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

async fn handle_key_event(
    app: &mut App,
    tui_manager: &mut TuiManager,
    git_manager: &mut GitManager,
    code: KeyCode,
    sync_tx: &mpsc::UnboundedSender<SyncEvent>,
) -> Result<()> {
    match app.state {
        AppState::ConfigReview => {
            match code {
                KeyCode::Enter => app.state = AppState::FileSelection,
                KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                _ => {}
            }
        }
        AppState::FileSelection => {
            if !app.loaded_changes {
                app.status_message = "正在加载提交历史...".to_string();
                match load_commits(&app.config, git_manager) {
                    Ok(commits) => {
                        app.set_commits(commits);
                        app.loaded_changes = true;
                        if app.commits.is_empty() {
                            app.status_message = "未发现任何相关提交历史".to_string();
                            app.state = AppState::Completed;
                        } else {
                            app.list_state.select(Some(0));
                        }
                    }
                    Err(e) => {
                        app.status_message = format!("加载提交失败: {}", e);
                        app.state = AppState::Completed;
                    }
                }
                return Ok(());
            }

            match code {
                KeyCode::Up => app.previous(),
                KeyCode::Down => app.next(),
                KeyCode::Char(' ') => app.toggle_commit_selection(),
                KeyCode::Char('a') => app.select_all(),
                KeyCode::Char('A') => app.deselect_all(),
                KeyCode::Enter => {
                    if app.get_selected_count() > 0 {
                        app.state = AppState::Confirmation;
                        app.current_confirmation = Some(ConfirmationAction::ExecuteSync);
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                _ => {}
            }
        }
        AppState::Confirmation => {
            if let Some(confirmation_type) = &app.current_confirmation {
                let message = get_confirmation_message(confirmation_type, &app.config)?;
                let result = tui_manager.show_confirmation(&message).map_err(SyncError::Anyhow)?;

                app.confirmation_result = Some(result);

                match confirmation_type {
                    ConfirmationAction::ExecuteSync => {
                        if result {
                            app.state = AppState::Progress;
                            app.start_time = std::time::Instant::now();
                            start_background_sync(app, git_manager, sync_tx.clone());
                        } else {
                            app.state = AppState::FileSelection;
                        }
                    }
                    _ => {}
                }
                app.current_confirmation = None;
            }
        }
        AppState::Progress => {
            // In progress, we might want to handle 'q' to abort in the future
            if code == KeyCode::Char('q') || code == KeyCode::Esc {
                // For now, just mark quit. Real-time abort needs more logic.
                app.should_quit = true;
            }
        }
        AppState::Completed => {
            if matches!(code, KeyCode::Enter | KeyCode::Char('q') | KeyCode::Esc) {
                app.should_quit = true;
            }
        }
    }
    Ok(())
}

fn handle_sync_event(app: &mut App, event: SyncEvent) {
    match event {
        SyncEvent::Progress { current, total, subject, status } => {
            app.progress = current as f64 / total as f64;
            app.status_message = format!("[{}] {}", status, subject);
        }
        SyncEvent::Completed(stats) => {
            app.progress = 1.0;
            app.end_time = Some(std::time::Instant::now());
            app.sync_stats = Some(stats.clone());
            app.status_message = format!(
                "同步完成: 总计 {}, 同步 {}, 跳过 {}",
                stats.total_commits,
                stats.synced_commits,
                stats.skipped_commits
            );
            app.state = AppState::Completed;
        }
        SyncEvent::Error(err) => {
            app.status_message = format!("同步失败: {}", err);
            app.state = AppState::Completed;
        }
    }
}

fn start_background_sync(
    app: &App,
    git_manager: &GitManager,
    tx: mpsc::UnboundedSender<SyncEvent>,
) {
    let sync_config = SyncConfig {
        subdir: app.config.subdir.clone(),
    };

    let selected_commits: Vec<_> = app.commits
        .iter()
        .zip(app.selected_commits.iter())
        .filter_map(|(commit, &selected)| if selected { Some(commit.clone()) } else { None })
        .collect();

    // Clone git_manager is not possible because it's not Clone, 
    // and Repository is not thread-safe. 
    // We need to recreate GitManager in the task or just move it if it's the last sync.
    // However, GitManager only contains metadata, it doesn't hold Repository long-term.
    // So we can clone the RepoInfo.
    
    let source_path = git_manager.source_repo_info.path.clone();
    let target_path = git_manager.target_repo_info.path.clone();
    let dry_run = app.config.dry_run;

    tokio::spawn(async move {
        match GitManager::new(&source_path, &target_path) {
            Ok(gm) => {
                let mut engine = SyncEngine::new(sync_config, dry_run);
                if let Err(e) = engine.sync_commits(&gm, &selected_commits, tx.clone()).await {
                    let _ = tx.send(SyncEvent::Error(e.to_string()));
                }
            }
            Err(e) => {
                let _ = tx.send(SyncEvent::Error(format!("Failed to initialize GitManager in background: {}", e)));
            }
        }
    });
}

fn load_commits(config: &Config, git_manager: &GitManager) -> Result<Vec<git::CommitInfo>> {
    let end_commit = config.end_commit.as_ref().map(|s| s.as_str()).unwrap_or("HEAD");
    let include_start = config.include_start.unwrap_or(true);
    let first_parent = config.no_merge.unwrap_or(true);

    git_manager.get_commits_in_range(
        &config.subdir,
        &config.start_commit,
        end_commit,
        include_start,
        first_parent,
    )
}

fn validate_config(config: &Config) -> Result<()> {
    if !config.source_repo.exists() {
        return Err(SyncError::PathNotFound(config.source_repo.clone()));
    }
    if !config.source_repo.join(".git").exists() {
        return Err(SyncError::NotARepository(config.source_repo.clone()));
    }
    if !config.target_repo.exists() {
        return Err(SyncError::PathNotFound(config.target_repo.clone()));
    }
    if !config.target_repo.join(".git").exists() {
        return Err(SyncError::NotARepository(config.target_repo.clone()));
    }

    let subdir_path = config.source_repo.join(&config.subdir);
    if !subdir_path.exists() {
        return Err(SyncError::PathNotFound(subdir_path));
    }

    Ok(())
}

fn get_confirmation_message(action: &ConfirmationAction, _config: &Config) -> Result<String> {
    match action {
        ConfirmationAction::ExecuteSync => Ok("确定要执行同步操作吗？".to_string()),
        ConfirmationAction::CreateBranch => Ok("是否创建新分支？".to_string()),
        ConfirmationAction::StashChanges => Ok("是否自动 Stash 变更？".to_string()),
        ConfirmationAction::IncludeStart => Ok("是否包含起始 commit 的变更？".to_string()),
        ConfirmationAction::ExcludeMerges => Ok("是否排除 merge 引入的变更？".to_string()),
        ConfirmationAction::SyncDelete => Ok("是否同步删除操作？".to_string()),
    }
}