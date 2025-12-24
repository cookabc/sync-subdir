use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{
        Block, Borders, Clear, Gauge, ListState, Paragraph, Wrap,
        Table, Row, Cell
    },
    Frame, Terminal,
};
use std::io::stdout;
use std::time::{Duration, Instant};

use crate::cli::Config;
use crate::git::CommitInfo;
use crate::sync::{SyncStats};

#[derive(Debug, Clone, PartialEq)]
pub enum AppState {
    ConfigReview,
    FileSelection,
    Progress,
    Confirmation,
    Completed,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ConfirmationAction {
    CreateBranch,
    StashChanges,
    IncludeStart,
    ExcludeMerges,
    SyncDelete,
    ExecuteSync,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct App {
    pub state: AppState,
    pub config: Config,
    pub commits: Vec<CommitInfo>,
    pub selected_commits: Vec<bool>,
    pub current_confirmation: Option<ConfirmationAction>,
    pub progress: f64,
    pub status_message: String,
    pub current_tab: usize,
    pub list_state: ListState,
    pub should_quit: bool,
    pub confirmation_result: Option<bool>,
    pub start_time: Instant,
    pub end_time: Option<Instant>,
    pub loaded_changes: bool,
    pub sync_stats: Option<SyncStats>,
}

impl App {
    pub fn new(config: Config) -> Self {
        Self {
            state: AppState::ConfigReview,
            config,
            commits: Vec::new(),
            selected_commits: Vec::new(),
            current_confirmation: None,
            progress: 0.0,
            status_message: String::new(),
            current_tab: 0,
            list_state: ListState::default(),
            should_quit: false,
            confirmation_result: None,
            start_time: Instant::now(),
            end_time: None,
            loaded_changes: false,
            sync_stats: None,
        }
    }

    pub fn set_commits(&mut self, commits: Vec<CommitInfo>) {
        let count = commits.len();
        self.commits = commits;
        self.selected_commits = vec![true; count];
    }

    pub fn next(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.commits.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    pub fn previous(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.commits.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    pub fn toggle_commit_selection(&mut self) {
        if let Some(i) = self.list_state.selected() {
            if i < self.selected_commits.len() {
                self.selected_commits[i] = !self.selected_commits[i];
            }
        }
    }

    pub fn select_all(&mut self) {
        self.selected_commits.fill(true);
    }

    pub fn deselect_all(&mut self) {
        self.selected_commits.fill(false);
    }

    pub fn get_selected_count(&self) -> usize {
        self.selected_commits.iter().filter(|&&selected| selected).count()
    }
}

pub struct TuiManager {
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
}

impl TuiManager {
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    pub fn draw(&mut self, app: &App) -> Result<()> {
        self.terminal.draw(|f| {
            match app.state {
                AppState::ConfigReview => Self::draw_config_review(f, app),
                AppState::FileSelection => Self::draw_file_selection(f, app),
                AppState::Progress => Self::draw_progress(f, app),
                AppState::Confirmation => Self::draw_confirmation(f, app),
                AppState::Completed => Self::draw_completed(f, app),
            }
        })?;
        Ok(())
    }

    fn draw_config_review(f: &mut Frame, app: &App) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(3),
            ])
            .split(f.size());

        // Title
        let title = Paragraph::new("配置审查")
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(title, chunks[0]);

        // Configuration table
        let config_rows = vec![
            Row::new(vec![
                Cell::from("源仓库"),
                Cell::from(app.config.source_repo.to_string_lossy()),
            ]),
            Row::new(vec![
                Cell::from("目标仓库"),
                Cell::from(app.config.target_repo.to_string_lossy()),
            ]),
            Row::new(vec![
                Cell::from("子目录"),
                Cell::from(app.config.subdir.clone()),
            ]),
            Row::new(vec![
                Cell::from("起始 Commit"),
                Cell::from(app.config.start_commit.clone()),
            ]),
            Row::new(vec![
                Cell::from("结束 Commit"),
                Cell::from(app.config.end_commit.clone().unwrap_or_else(|| "HEAD".to_string())),
            ]),
        ];

        let table = Table::new(config_rows)
            .widths(&[Constraint::Length(15), Constraint::Percentage(80)])
            .block(Block::default().borders(Borders::ALL).title("同步配置"))
            .style(Style::default().fg(Color::White));
        f.render_widget(table, chunks[1]);

        // Instructions
        let instructions = Paragraph::new("按 Enter 继续 | 按 q 退出")
            .style(Style::default().fg(Color::Gray))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(instructions, chunks[2]);
    }

    fn draw_file_selection(f: &mut Frame, app: &App) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(3),
            ])
            .split(f.size());

        // Header
        let header_text = format!(
            "待同步提交列表 (总计: {}, 已选择: {})",
            app.commits.len(),
            app.get_selected_count()
        );
        let header = Paragraph::new(header_text)
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(header, chunks[0]);

        // Commit Table
        let rows: Vec<Row> = app.commits.iter().enumerate().map(|(i, commit)| {
            let selected_symbol = if app.selected_commits[i] { "✓" } else { " " };
            let style = if Some(i) == app.list_state.selected() {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else if commit.is_merge {
                Style::default().fg(Color::Blue)
            } else {
                Style::default().fg(Color::White)
            };

            Row::new(vec![
                Cell::from(selected_symbol),
                Cell::from(commit.id[..7].to_string()),
                Cell::from(commit.subject.clone()),
                Cell::from(commit.author.clone()),
                Cell::from(commit.date.clone()),
            ]).style(style)
        }).collect();

        let table = Table::new(rows)
            .header(
                Row::new(vec![" ", "Hash", "Subject", "Author", "Date"])
                    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            )
            .widths(&[
                Constraint::Length(2),
                Constraint::Length(8),
                Constraint::Percentage(50),
                Constraint::Percentage(15),
                Constraint::Percentage(25),
            ])
            .block(Block::default().borders(Borders::ALL).title("提交详情"))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD));
        
        f.render_widget(table, chunks[1]);

        // Instructions
        let instructions = Paragraph::new(
            "↑/↓: 导航 | Space: 选择/取消 | a: 全选 | A: 取消全选 | Enter: 开始同步 | q: 退出"
        )
        .style(Style::default().fg(Color::Gray))
        .wrap(Wrap { trim: true });
        f.render_widget(instructions, chunks[2]);
    }

    fn draw_progress(f: &mut Frame, app: &App) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(5),
            ])
            .split(f.size());

        // Title
        let title = Paragraph::new("同步进度")
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(title, chunks[0]);

        // Progress bar
        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("进度"))
            .gauge_style(Style::default().fg(Color::Green).bg(Color::Gray))
            .percent((app.progress * 100.0) as u16);
        f.render_widget(gauge, chunks[1]);

        // Status message
        let status = Paragraph::new(app.status_message.clone())
            .style(Style::default().fg(Color::White))
            .block(Block::default().borders(Borders::ALL).title("当前操作"))
            .wrap(Wrap { trim: true });
        f.render_widget(status, chunks[2]);
    }

    fn draw_confirmation(f: &mut Frame, app: &App) {
        // Darken the background
        f.render_widget(Clear, f.size());

        let popup_area = centered_rect(60, 20, f.size());

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(popup_area);

        let confirmation_text = match app.current_confirmation {
            Some(ConfirmationAction::CreateBranch) => "是否创建新分支?",
            Some(ConfirmationAction::StashChanges) => "是否自动 Stash 变更?",
            Some(ConfirmationAction::IncludeStart) => "是否包含起始 commit 的变更?",
            Some(ConfirmationAction::ExcludeMerges) => "是否排除 merge 引入的变更?",
            Some(ConfirmationAction::SyncDelete) => "是否同步删除操作?",
            Some(ConfirmationAction::ExecuteSync) => "是否执行同步?",
            None => "确认操作?",
        };

        let title = Paragraph::new("确认")
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(title, chunks[0]);

        let message = Paragraph::new(confirmation_text)
            .style(Style::default().fg(Color::White))
            .block(Block::default().borders(Borders::ALL))
            .alignment(ratatui::layout::Alignment::Center)
            .wrap(Wrap { trim: true });
        f.render_widget(message, chunks[1]);

        let instructions = Paragraph::new("Y: 是 | N: 否")
            .style(Style::default().fg(Color::Gray))
            .block(Block::default().borders(Borders::ALL))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(instructions, chunks[2]);
    }

    fn draw_completed(f: &mut Frame, app: &App) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(3),
            ])
            .split(f.size());

        // Title
        let title = Paragraph::new("同步完成!")
            .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(title, chunks[0]);

        // Summary
        let elapsed = if let Some(end) = app.end_time {
            end.duration_since(app.start_time)
        } else {
            app.start_time.elapsed()
        };
        
        let summary_text = format!(
            "同步完成!\n\n状态消息: {}\n\n用时: {:.2} 秒\n\n按 Enter 退出",
            app.status_message,
            elapsed.as_secs_f32()
        );

        let summary = Paragraph::new(summary_text)
            .style(Style::default().fg(Color::White))
            .block(Block::default().borders(Borders::ALL).title("完成"))
            .wrap(Wrap { trim: true });
        f.render_widget(summary, chunks[1]);

        // Instructions
        let instructions = Paragraph::new("按 Enter 退出")
            .style(Style::default().fg(Color::Gray))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(instructions, chunks[2]);
    }

    pub fn show_confirmation(&mut self, message: &str) -> Result<bool> {
        let popup_area = centered_rect(60, 20, self.terminal.size()?);

        loop {
            self.terminal.draw(|f| {
                f.render_widget(Clear, f.size());

                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Min(3),
                        Constraint::Length(3),
                    ])
                    .split(popup_area);

                let title = Paragraph::new("确认")
                    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                    .block(Block::default().borders(Borders::ALL))
                    .alignment(ratatui::layout::Alignment::Center);
                f.render_widget(title, chunks[0]);

                let msg = Paragraph::new(message)
                    .style(Style::default().fg(Color::White))
                    .block(Block::default().borders(Borders::ALL))
                    .alignment(ratatui::layout::Alignment::Center)
                    .wrap(Wrap { trim: true });
                f.render_widget(msg, chunks[1]);

                let instructions = Paragraph::new("Y: 是 | N: 否 | ESC: 取消")
                    .style(Style::default().fg(Color::Gray))
                    .block(Block::default().borders(Borders::ALL))
                    .alignment(ratatui::layout::Alignment::Center);
                f.render_widget(instructions, chunks[2]);
            })?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(KeyEvent { code, .. }) = event::read()? {
                    match code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => return Ok(true),
                        KeyCode::Char('n') | KeyCode::Char('N') => return Ok(false),
                        KeyCode::Esc => return Ok(false),
                        _ => {}
                    }
                }
            }
        }
    }

}

impl Drop for TuiManager {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}