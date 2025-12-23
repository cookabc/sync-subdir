#!/bin/bash
#
# sync-subdir.sh - 子目录变更同步工具 (高性能 Git Patch 版)
#
# 将源仓库中某个目录或文件的变更，逐个 Commit 同步到目标仓库。
# 原理: 使用 git format-patch + git am 构建高效、非侵入式的同步管道。
#

# 颜色定义
readonly BLUE='\033[0;34m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly RED='\033[0;31m'
readonly NC='\033[0m'

# 日志函数
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $1"; }

show_help() {
    cat << EOF
${BLUE}Sync Subdir${NC} - 子目录高效同步工具

用法: $(basename "$0") [选项] <源路径> <目标仓库> <commit范围>

参数:
    源路径          源仓库中的目录或文件路径 (支持自动探测仓库根目录)
    目标仓库        目标 Git 仓库路径
    commit范围      Git Revision Range (例如: main, HEAD~5..HEAD, v1.0..v2.0)
                    注意: 单个 commit hash 等同于 "hash..当前分支"

选项:
    -t, --target-dir <dir>   同步到目标仓库的指定子目录 (默认: 根目录)
    -b, --branch <branch>    切换目标仓库到指定分支 (不存在则自动创建)
    -s, --source-branch <br> 显式指定源仓库分支 (默认: 自动探测当前分支)
    --no-first-parent        包含合并进来的所有提交 (默认: 仅跟随第一亲本)
    --stash                  同步前自动 stash 目标仓库未提交的变更
    --continue               解决冲突后，继续未完成的同步
    --abort                  终止当前的同步并回滚
    --dry-run                预览模式，仅列出待同步提交
    -h, --help               显示帮助信息

示例:
    # 同步某个子目录的最近 10 个 commit
    $(basename "$0") ./source-repo/packages/utils ./target-repo HEAD~10

    # 同步单个文件到目标仓库
    $(basename "$0") ./source-repo/src/Main.java ./target-repo main
EOF
}

# ------------------------------------------------------------------------------
# 选项解析
# ------------------------------------------------------------------------------

TARGET_SUBDIR=""
TARGET_BRANCH=""
SOURCE_BRANCH=""
FIRST_PARENT=true
DRY_RUN=false
AUTO_STASH=false
MODE="sync" # sync, continue, abort
POSITIONAL_ARGS=()

while [[ $# -gt 0 ]]; do
    case $1 in
        -t|--target-dir)    TARGET_SUBDIR="$2"; shift 2 ;;
        -b|--branch)        TARGET_BRANCH="$2"; shift 2 ;;
        -s|--source-branch) SOURCE_BRANCH="$2"; shift 2 ;;
        --first-parent)     FIRST_PARENT=true; shift ;;
        --no-first-parent)  FIRST_PARENT=false; shift ;;
        --stash)            AUTO_STASH=true; shift ;;
        --continue)         MODE="continue"; shift ;;
        --abort)            MODE="abort"; shift ;;
        --dry-run)          DRY_RUN=true; shift ;;
        -y|--yes)           shift ;; # 兼容旧接口
        -h|--help)          show_help; exit 0 ;;
        -*)                 log_error "未知选项: $1"; show_help; exit 1 ;;
        *)                  POSITIONAL_ARGS+=("$1"); shift ;;
    esac
done

set -- "${POSITIONAL_ARGS[@]}"

# ------------------------------------------------------------------------------
# 工具函数
# ------------------------------------------------------------------------------

# 获取绝对路径
get_abs_path() {
    local path="$1"
    if [[ -d "$path" ]]; then
        (cd "$path" && pwd)
    elif [[ -f "$path" ]]; then
        echo "$(cd "$(dirname "$path")" && pwd)/$(basename "$path")"
    else
        realpath "$path" 2>/dev/null || echo "$path"
    fi
}

# 交互式询问
ask_user() {
    local question="$1" default="$2"
    local prompt="$question "
    [[ "$default" == "y" ]] && prompt+="[Y/n] " || prompt+="[y/N] "
    
    # 从 tty 读取，防止干扰管道
    read -p "$prompt" -n 1 -r < /dev/tty
    echo ""
    [[ -z "$REPLY" ]] && REPLY="$default"
    [[ $REPLY =~ ^[Yy]$ ]] && return 0 || return 1
}

# ------------------------------------------------------------------------------
# 核心逻辑
# ------------------------------------------------------------------------------

main() {
    # 1. 检查特殊模式 (Continue / Abort)
    if [[ "$MODE" != "sync" ]]; then
        local target="${1:-$(pwd)}"
        cd "$target" 2>/dev/null || { log_error "无效的目标仓库路径: $target"; exit 1; }
        if [[ "$MODE" == "continue" ]]; then
            log_info "正在继续之前的同步..."
            git am --continue
        else
            log_warn "正在终止同步并回滚..."
            git am --abort
        fi
        exit $?
    fi

    # 2. 验证参数
    if [[ $# -lt 3 ]]; then
        log_error "参数不足"
        show_help
        exit 1
    fi

    log_info "正在初始化同步任务..."

    local raw_src="$1"
    local raw_dest="$2"
    local rev_range="$3"

    # 解析路径
    local source_path=$(get_abs_path "$raw_src")
    local target_repo=$(get_abs_path "$raw_dest")
    
    [[ ! -d "$target_repo/.git" ]] && { log_error "目标仓库无效: $target_repo"; exit 1; }

    # 探测源仓库和子目录
    local source_dir_to_check="$source_path"
    [[ ! -d "$source_path" ]] && source_dir_to_check=$(dirname "$source_path")
    
    cd "$source_dir_to_check" 2>/dev/null || { log_error "无法访问源路径: $source_path"; exit 1; }
    local source_repo=$(git rev-parse --show-toplevel 2>/dev/null)
    local source_subdir=$(git rev-parse --show-prefix)
    
    # 处理单文件情况
    if [[ -f "$source_path" ]]; then
        source_subdir="${source_subdir}$(basename "$source_path")"
    fi
    source_subdir="${source_subdir%/}"

    [[ -z "$source_repo" ]] && { log_error "源路径不在 Git 仓库中: $source_path"; exit 1; }

    log_info "源仓库: $source_repo"
    log_info "同步内容: ${source_subdir:-. (根目录)}"
    log_info "目标仓库: $target_repo"

    # 3. 准备目标仓库
    cd "$target_repo" || exit 1
    
    # 状态检查
    if [[ -d .git/rebase-apply ]]; then
        log_error "目标仓库处于同步中断状态。请先解决冲突或使用 --abort"
        exit 1
    fi

    if [[ -n $(git status --porcelain) ]]; then
        if $AUTO_STASH; then
            log_info "正在自动 Stash 目标仓库变更..."
            git stash push -m "sync-subdir auto stash" --quiet
        else
            log_error "目标仓库有未提交变更。建议先清理或使用 --stash"
            exit 1
        fi
    fi

    # 分支处理
    if [[ -n "$TARGET_BRANCH" ]]; then
        if git rev-parse --verify "$TARGET_BRANCH" >/dev/null 2>&1; then
            git checkout "$TARGET_BRANCH" --quiet
        else
            log_info "在目标仓库创建新分支: $TARGET_BRANCH"
            git checkout -b "$TARGET_BRANCH" --quiet
        fi
    fi

    # 4. 分析与生成补丁
    local patch_dir=$(mktemp -d "/tmp/sync-subdir-patches-XXXXXX")
    trap "rm -rf $patch_dir" EXIT

    # 分支解析逻辑
    cd "$source_repo" || exit 1
    local current_src_br=$(git rev-parse --abbrev-ref HEAD)
    local effective_src_br="${SOURCE_BRANCH:-$current_src_br}"
    
    # 如果范围包含 HEAD，替换为明确的分支名以确保稳定性
    if [[ "$rev_range" == *"HEAD"* ]]; then
        log_info "锁定源分支: $effective_src_br (解析自 HEAD)"
        rev_range="${rev_range/HEAD/$effective_src_br}"
    elif [[ "$rev_range" != *".."* ]]; then
        # 默认从该点到分支 HEAD
        rev_range="${rev_range}..${effective_src_br}"
    fi

    log_info "正在分析变更逻辑 ($rev_range)..."
    [[ "$FIRST_PARENT" == "true" ]] && log_info "配置: 仅跟随第一亲本 (First Parent)"
    
    # 生成 Patch 列表
    (
        local fp_args=("-o" "$patch_dir" "--binary" "--full-index" "--relative=$source_subdir")
        [[ "$FIRST_PARENT" == "true" ]] && fp_args+=("--first-parent")
        
        git format-patch "${fp_args[@]}" "$rev_range" -- "$source_subdir" > /dev/null
    )

    local patches=($(ls "$patch_dir"/*.patch 2>/dev/null | sort))
    if [[ ${#patches[@]} -eq 0 ]]; then
        log_warn "未检测到针对指定内容的任何变更。"
        exit 0
    fi

    log_info "共发现 ${#patches[@]} 个待同步 Commit。"

    # 预览模式
    if $DRY_RUN; then
        echo -e "\n--- 待同步列表 ---"
        for p in "${patches[@]}"; do 
            grep "^Subject: " "$p" | head -1 | sed 's/Subject: \[PATCH.*\] //g'
        done
        log_success "\n预览完成，未执行实际变更。"
        exit 0
    fi

    # 5. 执行补丁应用
    cd "$target_repo" || exit 1
    local am_args=("--3way" "--committer-date-is-author-date")
    [[ -n "$TARGET_SUBDIR" ]] && am_args+=("--directory=$TARGET_SUBDIR")

    local count=0
    for p in "${patches[@]}"; do
        ((count++))
        local subj=$(grep "^Subject: " "$p" | head -1 | sed 's/Subject: \[PATCH.*\] //g')
        printf "[%d/%d] Applying: %s... " "$count" "${#patches[@]}" "${subj:0:40}"
        
        if git am "${am_args[@]}" "$p" > /dev/null 2>&1; then
            echo -e "${GREEN}完成${NC}"
        else
            # 处理空补丁 (该 Commit 虽然在范围内，但没改目标子目录)
            if [[ -d .git/rebase-apply ]] && ! git status --porcelain | grep -q "^M"; then
                echo -e "${YELLOW}空补丁${NC}"
                if ask_user "  --> 提交无实质内容变更，是否跳过?" "y"; then
                    git am --skip > /dev/null 2>&1
                    continue
                else
                    log_error "操作已由用户中止。"
                    exit 1
                fi
            else
                # 真正的合并冲突
                echo -e "${RED}失败${NC}"
                log_error "遇到合并冲突！请在目标仓库手动解决后运行: $(basename "$0") --continue"
                exit 1
            fi
        fi
    done

    log_success "\n所有变更同步成功！"
}

main "$@"
