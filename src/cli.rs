use clap::{Arg, ArgMatches, Command};
use std::path::PathBuf;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Config {
    pub source_repo: PathBuf,
    pub subdir: String,
    pub target_repo: PathBuf,
    pub start_commit: String,
    pub source_branch: Option<String>,
    pub target_branch: Option<String>,
    pub end_commit: Option<String>,
    pub create_branch: Option<bool>,
    pub include_start: Option<bool>,
    pub no_merge: Option<bool>,
    pub sync_delete: Option<bool>,
    pub auto_stash: Option<bool>,
    pub dry_run: bool,
    pub verbose: bool,
}

impl Config {
    pub fn from_matches(matches: ArgMatches) -> anyhow::Result<Self> {
        let source_repo = matches
            .get_one::<String>("source_repo")
            .ok_or_else(|| anyhow::anyhow!("Missing source repository path"))?;
        let subdir = matches
            .get_one::<String>("subdir")
            .ok_or_else(|| anyhow::anyhow!("Missing subdirectory name"))?;
        let target_repo = matches
            .get_one::<String>("target_repo")
            .ok_or_else(|| anyhow::anyhow!("Missing target repository path"))?;
        let start_commit = matches
            .get_one::<String>("start_commit")
            .ok_or_else(|| anyhow::anyhow!("Missing start commit"))?;

        Ok(Self {
            source_repo: PathBuf::from(source_repo),
            subdir: subdir.to_string(),
            target_repo: PathBuf::from(target_repo),
            start_commit: start_commit.to_string(),
            source_branch: matches.get_one::<String>("source_branch").cloned(),
            target_branch: matches.get_one::<String>("target_branch").cloned(),
            end_commit: matches.get_one::<String>("end_commit").cloned(),
            create_branch: matches.get_flag("create_branch").then_some(true)
                .or(matches.get_flag("no_create_branch").then_some(false)),
            include_start: matches.get_flag("include_start").then_some(true)
                .or(matches.get_flag("no_include_start").then_some(false)),
            no_merge: matches.get_flag("no_merge").then_some(true),
            sync_delete: matches.get_flag("delete").then_some(true)
                .or(matches.get_flag("no_delete").then_some(false)),
            auto_stash: matches.get_flag("stash").then_some(true),
            dry_run: matches.get_flag("dry_run"),
            verbose: matches.get_flag("verbose"),
        })
    }

    pub fn get_default_target_branch(&self) -> String {
        self.target_branch
            .clone()
            .unwrap_or_else(|| self.source_branch.clone().unwrap_or_else(|| "main".to_string()))
    }
}

pub fn build_cli() -> Command {
    Command::new("sync-subdir")
        .version("0.1.0")
        .author("Claude <noreply@anthropic.com>")
        .about("A TUI tool for syncing subdirectory changes between Git repositories")
        .long_about(
            "将源仓库中某个子目录的变更同步到独立的目标仓库。\n\n\
             这个工具提供了交互式 TUI 界面，支持分支管理、commit 范围选择、\n\
             merge 排除、删除操作同步等功能。",
        )
        .arg(
            Arg::new("source_repo")
                .help("源 Git 仓库路径")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("subdir")
                .help("源仓库中要同步的子目录名称")
                .required(true)
                .index(2),
        )
        .arg(
            Arg::new("target_repo")
                .help("目标 Git 仓库路径")
                .required(true)
                .index(3),
        )
        .arg(
            Arg::new("start_commit")
                .help("起始 commit hash")
                .required(true)
                .index(4),
        )
        .arg(
            Arg::new("source_branch")
                .long("source-branch")
                .short('b')
                .help("源仓库分支")
                .value_name("分支"),
        )
        .arg(
            Arg::new("target_branch")
                .long("target-branch")
                .short('t')
                .help("目标仓库分支")
                .value_name("分支"),
        )
        .arg(
            Arg::new("end_commit")
                .long("end")
                .short('e')
                .help("结束 commit (默认: HEAD)")
                .value_name("commit"),
        )
        .arg(
            Arg::new("create_branch")
                .long("create-branch")
                .short('c')
                .help("自动创建目标分支")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no_create_branch")
                .long("no-create-branch")
                .help("禁止自动创建目标分支")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with("create_branch"),
        )
        .arg(
            Arg::new("include_start")
                .long("include-start")
                .short('i')
                .help("包含起始 commit 的变更")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no_include_start")
                .long("no-include-start")
                .help("不包含起始 commit 的变更")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with("include_start"),
        )
        .arg(
            Arg::new("no_merge")
                .long("no-merge")
                .short('n')
                .help("排除 merge 引入的变更")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("delete")
                .long("delete")
                .help("同步删除操作")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no_delete")
                .long("no-delete")
                .help("不同步删除操作")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with("delete"),
        )
        .arg(
            Arg::new("stash")
                .long("stash")
                .help("自动 stash 目标仓库未提交变更")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("dry_run")
                .long("dry-run")
                .short('d')
                .help("预览模式，不实际执行")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("verbose")
                .long("verbose")
                .short('v')
                .help("详细输出")
                .action(clap::ArgAction::SetTrue),
        )
        .after_help(
            "示例:\n  \
             sync-subdir /repo/main submodule /repo/sub abc123\n  \
             sync-subdir -b feature/x -n /repo/main submodule /repo/sub abc123",
        )
}