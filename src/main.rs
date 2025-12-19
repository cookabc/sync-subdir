mod cli;
mod git;
mod tui;
mod sync;

use anyhow::{anyhow, Context, Result};
use crossterm::event::KeyCode;
use tracing::{info, warn, error, Level};
use tracing_subscriber;

use cli::{build_cli, Config};
use git::GitManager;
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
    let config = Config::from_matches(matches)?;

    // Validate configuration
    validate_config(&config)?;

    // Initialize Git manager
    let mut git_manager = GitManager::new(&config.source_repo, &config.target_repo)
        .context("Failed to initialize Git manager")?;

    // Validate commits
    git_manager.validate_commit(true, &config.start_commit)?;
    if let Some(ref end_commit) = config.end_commit {
        git_manager.validate_commit(true, end_commit)?;
    }

    // Get current branches for restoration later
    let _source_original_branch = git_manager.source_repo_info.original_branch.clone();
    let _target_original_branch = git_manager.target_repo_info.original_branch.clone();

    // Switch branches if specified
    if let Some(ref source_branch) = config.source_branch {
        git_manager.switch_branch(true, source_branch)?;
    }

    let target_branch = config.get_default_target_branch();

    // Handle target branch creation/switching
    if !git_manager.get_repository(false)?.revparse_single(&format!("refs/heads/{}", target_branch)).is_ok() {
        if config.create_branch.unwrap_or(true) {
            git_manager.create_branch(false, &target_branch)?;
        } else {
            return Err(anyhow!("Target branch '{}' does not exist", target_branch));
        }
    } else {
        git_manager.switch_branch(false, &target_branch)?;
    }

    // Handle uncommitted changes in target repo
    let mut stashed = false;
    if git_manager.has_uncommitted_changes(false)? {
        if config.auto_stash.unwrap_or(true) {
            let stash_message = format!("sync-subdir auto stash {}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
            git_manager.stash_changes(false, &stash_message)?;
            stashed = true;
        } else {
            return Err(anyhow!("Target repository has uncommitted changes"));
        }
    }

    // Initialize TUI
    let mut tui_manager = TuiManager::new()
        .context("Failed to initialize TUI")?;

    let mut app = App::new(config.clone());

    // Run the application
    let result = run_application(&mut app, &mut tui_manager, &mut git_manager).await;

    // Cleanup
    if stashed {
        info!("Restoring stashed changes...");
        if let Err(e) = git_manager.pop_stash(false) {
            warn!("Failed to restore stashed changes: {}", e);
        }
    }

    // Restore original branches
    info!("Restoring original branches...");
    if let Err(e) = git_manager.restore_original_branches() {
        warn!("Failed to restore original branches: {}", e);
    }

    result
}

async fn run_application(
    app: &mut App,
    tui_manager: &mut TuiManager,
    git_manager: &mut GitManager,
) -> Result<()> {
    loop {
        tui_manager.draw(app)?;

        match app.state {
            AppState::ConfigReview => {
                let key = tui_manager.handle_events()?;
                if key != KeyCode::Null {
                    info!("Key received in ConfigReview: {:?}", key);
                }
                match key {
                    KeyCode::Enter => {
                        info!("Transitioning to FileSelection state");
                        app.state = AppState::FileSelection;
                    }
                    KeyCode::Char('q') | KeyCode::Esc => {
                        info!("Quit requested from ConfigReview");
                        app.should_quit = true;
                        break;
                    }
                    _ => {}
                }
            }
            AppState::FileSelection => {
                // Load file changes on first entry
                if !app.loaded_changes {
                    info!("Loading file changes...");
                    app.status_message = "正在加载文件变更...".to_string();
                    tui_manager.draw(app)?;
                    
                    match load_file_changes(&app.config, git_manager) {
                        Ok(changes) => {
                            info!("Successfully loaded {} changes", changes.len());
                            app.set_file_changes(changes);
                            app.loaded_changes = true;
                            if !app.file_changes.is_empty() {
                                app.list_state.select(Some(0));
                            } else {
                                app.status_message = "未发现任何变更".to_string();
                                app.state = AppState::Completed;
                            }
                        }
                        Err(e) => {
                            error!("Failed to load changes: {}", e);
                            app.status_message = format!("加载变更失败: {}", e);
                            app.state = AppState::Completed;
                        }
                    }
                    tui_manager.draw(app)?;
                }

                let key = tui_manager.handle_events()?;
                if key != KeyCode::Null {
                    info!("Key received in FileSelection: {:?}", key);
                }
                match key {
                    KeyCode::Up => app.previous(),
                    KeyCode::Down => app.next(),
                    KeyCode::Char(' ') => app.toggle_file_selection(),
                    KeyCode::Char('a') => app.select_all(),
                    KeyCode::Char('A') => app.deselect_all(),
                    KeyCode::Enter => {
                        if app.get_selected_count() > 0 {
                            info!("Transitioning to Confirmation state with {} files", app.get_selected_count());
                            app.state = AppState::Confirmation;
                            app.current_confirmation = Some(ConfirmationAction::ExecuteSync);
                        } else {
                            warn!("Enter pressed but no files selected");
                        }
                    }
                    KeyCode::Char('q') | KeyCode::Esc => {
                        info!("Quit requested from FileSelection");
                        app.should_quit = true;
                        break;
                    }
                    _ => {}
                }
            }
            AppState::Confirmation => {
                if let Some(confirmation_type) = &app.current_confirmation {
                    let message = get_confirmation_message(confirmation_type, &app.config)?;
                    let result = tui_manager.show_confirmation(&message)?;

                    app.confirmation_result = Some(result);

                    // Handle confirmation result
                    match confirmation_type {
                        ConfirmationAction::ExecuteSync => {
                            if result {
                                app.state = AppState::Progress;
                                app.start_time = std::time::Instant::now();
                                perform_sync(app, tui_manager, git_manager).await?;
                                app.state = AppState::Completed;
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
                // Progress is handled in perform_sync logic which transitions to Completed
                // Just draw to keep the UI updated if any async updates come through
                tui_manager.draw(app)?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            AppState::Completed => {
                let key = tui_manager.handle_events()?;
                if key != KeyCode::Null {
                    info!("Key received in Completed: {:?}", key);
                }
                match key {
                    KeyCode::Enter | KeyCode::Char('q') | KeyCode::Esc => {
                        info!("Exiting after completion");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

async fn perform_sync(
    app: &mut App,
    tui_manager: &mut TuiManager,
    _git_manager: &mut GitManager,
) -> Result<()> {
    let sync_config = SyncConfig {
        source_repo: app.config.source_repo.clone(),
        target_repo: app.config.target_repo.clone(),
        subdir: app.config.subdir.clone(),
        sync_delete: app.config.sync_delete.unwrap_or(true),
        verbose: app.config.verbose,
    };

    let mut sync_engine = SyncEngine::new(sync_config, app.config.dry_run);

    // Validate paths
    sync_engine.validate_paths()?;

    // Filter selected changes
    let selected_changes: Vec<_> = app.file_changes
        .iter()
        .zip(app.selected_files.iter())
        .filter_map(|(change, &selected)| if selected { Some(change.clone()) } else { None })
        .collect();

    let selected_files: Vec<bool> = vec![true; selected_changes.len()];

    // Perform sync with progress callback
    let stats = sync_engine.sync_files(
        &selected_changes,
        &selected_files,
        |current, total, file_path, status| {
            app.progress = current as f64 / total as f64;
            app.status_message = format!("{} - {}", file_path, status);
            let _ = tui_manager.draw(app);
        },
    ).await?;

    // Update final status
    app.progress = 1.0;
    app.end_time = Some(std::time::Instant::now());
    app.status_message = format!(
        "同步完成: 总计 {}, 同步 {}, 删除 {}, 失败 {}, 跳过 {}",
        stats.total_files,
        stats.synced_files,
        stats.deleted_files,
        stats.failed_files,
        stats.skipped_files
    );

    Ok(())
}

fn load_file_changes(config: &Config, git_manager: &GitManager) -> Result<Vec<git::FileChange>> {
    let end_commit = config.end_commit.as_ref().map(|s| s.as_str()).unwrap_or("HEAD");
    let include_start = config.include_start.unwrap_or(true);
    let exclude_merges = config.no_merge.unwrap_or(false);

    let changes = git_manager.get_file_changes(
        &config.subdir,
        &config.start_commit,
        end_commit,
        include_start,
        exclude_merges,
    )?;

    if changes.is_empty() {
        warn!("No file changes found in the specified range");
    } else {
        info!("Found {} file changes to sync", changes.len());
    }

    Ok(changes)
}

fn validate_config(config: &Config) -> Result<()> {
    // Validate source repository
    if !config.source_repo.exists() {
        return Err(anyhow!("Source repository does not exist: {}", config.source_repo.display()));
    }

    if !config.source_repo.join(".git").exists() {
        return Err(anyhow!("Source repository is not a git repository: {}", config.source_repo.display()));
    }

    // Validate target repository
    if !config.target_repo.exists() {
        return Err(anyhow!("Target repository does not exist: {}", config.target_repo.display()));
    }

    if !config.target_repo.join(".git").exists() {
        return Err(anyhow!("Target repository is not a git repository: {}", config.target_repo.display()));
    }

    // Validate subdirectory
    let subdir_path = config.source_repo.join(&config.subdir);
    if !subdir_path.exists() {
        return Err(anyhow!("Subdirectory does not exist in source repository: {}", subdir_path.display()));
    }

    Ok(())
}

fn get_confirmation_message(action: &ConfirmationAction, _config: &Config) -> Result<String> {
    match action {
        ConfirmationAction::ExecuteSync => {
            Ok("确定要执行同步操作吗？".to_string())
        }
        ConfirmationAction::CreateBranch => {
            Ok("是否创建新分支？".to_string())
        }
        ConfirmationAction::StashChanges => {
            Ok("是否自动 Stash 变更？".to_string())
        }
        ConfirmationAction::IncludeStart => {
            Ok("是否包含起始 commit 的变更？".to_string())
        }
        ConfirmationAction::ExcludeMerges => {
            Ok("是否排除 merge 引入的变更？".to_string())
        }
        ConfirmationAction::SyncDelete => {
            Ok("是否同步删除操作？".to_string())
        }
    }
}