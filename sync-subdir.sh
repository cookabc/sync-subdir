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
readonly CYAN='\033[0;36m'
readonly GRAY='\033[0;90m'
readonly NC='\033[0m'

# 状态目录名
readonly STATE_DIR_NAME="sync-subdir-state"
readonly LOG_FILE_NAME="sync-subdir.log"

# 日志函数
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
log_detail()  { echo -e "${GRAY}      $1${NC}"; }

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
    --include-start          包含起始 commit (自动将 A..B 转换为 A^..B)
    --stash                  同步前自动 stash 目标仓库未提交的变更
    --skip-empty             自动跳过空补丁，无需交互确认
    --interactive, -i        交互模式，逐个确认是否同步每个 commit
    --continue               解决冲突后，继续未完成的同步
    --abort                  终止当前的同步并回滚
    --dry-run                预览模式，仅列出待同步提交 (含详细信息)
    -h, --help               显示帮助信息

示例:
    # 同步某个子目录的最近 10 个 commit
    $(basename "$0") ./source-repo/packages/utils ./target-repo HEAD~10

    # 同步单个文件到目标仓库 (包含起始 commit)
    $(basename "$0") --include-start ./source-repo/src/Main.java ./target-repo abc123..def456

    # 非交互式同步，自动跳过空补丁
    $(basename "$0") --skip-empty ./source-repo/lib ./target-repo main
EOF
}

# ------------------------------------------------------------------------------
# 选项解析
# ------------------------------------------------------------------------------

TARGET_SUBDIR=""
TARGET_BRANCH=""
SOURCE_BRANCH=""
FIRST_PARENT=true
INCLUDE_START=false
DRY_RUN=false
AUTO_STASH=false
SKIP_EMPTY=false
INTERACTIVE=false
MODE="sync" # sync, continue, abort
POSITIONAL_ARGS=()

while [[ $# -gt 0 ]]; do
    case $1 in
        -t|--target-dir)    TARGET_SUBDIR="$2"; shift 2 ;;
        -b|--branch)        TARGET_BRANCH="$2"; shift 2 ;;
        -s|--source-branch) SOURCE_BRANCH="$2"; shift 2 ;;
        --first-parent)     FIRST_PARENT=true; shift ;;
        --no-first-parent)  FIRST_PARENT=false; shift ;;
        --include-start)    INCLUDE_START=true; shift ;;
        --stash)            AUTO_STASH=true; shift ;;
        --skip-empty)       SKIP_EMPTY=true; shift ;;
        -i|--interactive)   INTERACTIVE=true; shift ;;
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

# 获取状态目录路径
get_state_dir() {
    local target_repo="$1"
    echo "$target_repo/.git/$STATE_DIR_NAME"
}

# 获取日志文件路径
get_log_file() {
    local target_repo="$1"
    echo "$target_repo/.git/$LOG_FILE_NAME"
}

# 写入同步日志
write_log() {
    local target_repo="$1"
    local message="$2"
    local log_file=$(get_log_file "$target_repo")
    local timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[$timestamp] $message" >> "$log_file"
}

# 保存同步状态
save_state() {
    local target_repo="$1"
    local source_repo="$2"
    local source_subdir="$3"
    local rev_range="$4"
    local patch_dir="$5"
    
    local state_dir=$(get_state_dir "$target_repo")
    mkdir -p "$state_dir/patches"
    
    # 保存元信息
    cat > "$state_dir/info" << EOF
SOURCE_REPO=$source_repo
SOURCE_SUBDIR=$source_subdir
REV_RANGE=$rev_range
TARGET_SUBDIR=$TARGET_SUBDIR
FIRST_PARENT=$FIRST_PARENT
SKIP_EMPTY=$SKIP_EMPTY
INTERACTIVE=$INTERACTIVE
STARTED_AT=$(date '+%Y-%m-%d %H:%M:%S')
EOF
    
    # 复制 patches
    cp "$patch_dir"/*.patch "$state_dir/patches/" 2>/dev/null
    
    # 初始化进度
    echo "0" > "$state_dir/progress"
}

# 加载同步状态
load_state() {
    local target_repo="$1"
    local state_dir=$(get_state_dir "$target_repo")
    
    if [[ ! -f "$state_dir/info" ]]; then
        return 1
    fi
    
    source "$state_dir/info"
    return 0
}

# 更新进度
update_progress() {
    local target_repo="$1"
    local current="$2"
    local state_dir=$(get_state_dir "$target_repo")
    echo "$current" > "$state_dir/progress"
}

# 获取当前进度
get_progress() {
    local target_repo="$1"
    local state_dir=$(get_state_dir "$target_repo")
    if [[ -f "$state_dir/progress" ]]; then
        cat "$state_dir/progress"
    else
        echo "0"
    fi
}

# 清理状态
cleanup_state() {
    local target_repo="$1"
    local state_dir=$(get_state_dir "$target_repo")
    rm -rf "$state_dir"
}

# 检查是否有未完成的同步
check_pending_sync() {
    local target_repo="$1"
    local state_dir=$(get_state_dir "$target_repo")
    
    if [[ -d "$state_dir" && -f "$state_dir/info" ]]; then
        return 0  # 有未完成的同步
    fi
    return 1
}

# 显示未完成同步信息
show_pending_sync_info() {
    local target_repo="$1"
    local state_dir=$(get_state_dir "$target_repo")
    
    if load_state "$target_repo"; then
        log_warn "检测到未完成的同步任务:"
        log_detail "源仓库: $SOURCE_REPO"
        log_detail "同步内容: $SOURCE_SUBDIR"
        log_detail "Commit 范围: $REV_RANGE"
        log_detail "开始时间: $STARTED_AT"
        
        local progress=$(get_progress "$target_repo")
        local total=$(ls "$state_dir/patches/"*.patch 2>/dev/null | wc -l | tr -d ' ')
        log_detail "进度: $progress / $total"
        
        echo ""
        log_info "使用 --continue 继续同步，或 --abort 终止并回滚"
        return 0
    fi
    return 1
}

# 获取 commit 涉及的文件数
get_patch_file_count() {
    local patch_file="$1"
    grep -c "^diff --git" "$patch_file" 2>/dev/null || echo "0"
}

# 检查是否可能是空补丁
check_if_likely_empty() {
    local patch_file="$1"
    local source_repo="$2"
    local source_subdir="$3"
    
    # 获取 commit hash
    local commit_hash=$(grep "^From " "$patch_file" | head -1 | awk '{print $2}')
    if [[ -z "$commit_hash" ]]; then
        return 1
    fi
    
    # 检查该 commit 是否真的修改了目标子目录
    cd "$source_repo" || return 1
    local changed_in_subdir=$(git show --name-only --pretty=format: "$commit_hash" -- "$source_subdir" 2>/dev/null | grep -c .)
    
    if [[ "$changed_in_subdir" -eq 0 ]]; then
        return 0  # 可能是空补丁
    fi
    return 1
}

# ------------------------------------------------------------------------------
# 核心逻辑
# ------------------------------------------------------------------------------

# Continue 模式处理
handle_continue() {
    local target="${1:-$(pwd)}"
    target=$(get_abs_path "$target")
    
    cd "$target" 2>/dev/null || { log_error "无效的目标仓库路径: $target"; exit 1; }
    
    local state_dir=$(get_state_dir "$target")
    
    # 首先尝试恢复 git am 状态
    if [[ -d .git/rebase-apply ]]; then
        log_info "正在继续 git am..."
        if ! git am --continue; then
            log_error "git am --continue 失败，请先解决冲突"
            exit 1
        fi
    fi
    
    # 检查是否有保存的状态
    if ! load_state "$target"; then
        log_success "同步已完成，无需继续。"
        exit 0
    fi
    
    log_info "正在继续之前的同步..."
    log_detail "源仓库: $SOURCE_REPO"
    log_detail "同步内容: $SOURCE_SUBDIR"
    
    # 获取剩余的 patches
    local patches=($(ls "$state_dir/patches/"*.patch 2>/dev/null | sort))
    local progress=$(get_progress "$target")
    local total=${#patches[@]}
    
    if [[ $progress -ge $total ]]; then
        log_success "所有变更已同步完成！"
        cleanup_state "$target"
        write_log "$target" "同步完成 (continue): $SOURCE_SUBDIR from $SOURCE_REPO"
        exit 0
    fi
    
    log_info "从第 $((progress + 1)) 个 commit 继续 (共 $total 个)"
    
    # 准备 am 参数
    local am_args=("--3way" "--committer-date-is-author-date")
    [[ -n "$TARGET_SUBDIR" ]] && am_args+=("--directory=$TARGET_SUBDIR")
    
    # 继续应用剩余 patches
    local count=$progress
    for ((i=progress; i<total; i++)); do
        local p="${patches[$i]}"
        ((count++))
        local subj=$(grep "^Subject: " "$p" | head -1 | sed 's/Subject: \[PATCH.*\] //g')
        printf "[%d/%d] Applying: %s... " "$count" "$total" "${subj:0:40}"
        
        if git am "${am_args[@]}" "$p" > /dev/null 2>&1; then
            echo -e "${GREEN}完成${NC}"
            update_progress "$target" "$count"
        else
            if [[ -d .git/rebase-apply ]] && ! git status --porcelain | grep -q "^[UD]"; then
                echo -e "${YELLOW}空补丁${NC}"
                if $SKIP_EMPTY || ask_user "  --> 提交无实质内容变更，是否跳过?" "y"; then
                    git am --skip > /dev/null 2>&1
                    update_progress "$target" "$count"
                    continue
                else
                    log_error "操作已由用户中止。"
                    exit 1
                fi
            else
                echo -e "${RED}失败${NC}"
                update_progress "$target" "$((count - 1))"
                log_error "遇到合并冲突！请解决后运行:"
                log_detail "cd $target && $(basename "$0") --continue"
                exit 1
            fi
        fi
    done
    
    log_success "\n所有变更同步成功！"
    cleanup_state "$target"
    write_log "$target" "同步完成 (continue): $SOURCE_SUBDIR from $SOURCE_REPO, $total commits"
}

# Abort 模式处理
handle_abort() {
    local target="${1:-$(pwd)}"
    target=$(get_abs_path "$target")
    
    cd "$target" 2>/dev/null || { log_error "无效的目标仓库路径: $target"; exit 1; }
    
    log_warn "正在终止同步并回滚..."
    
    # 终止 git am
    if [[ -d .git/rebase-apply ]]; then
        git am --abort
    fi
    
    # 清理状态
    cleanup_state "$target"
    write_log "$target" "同步已终止 (abort)"
    
    log_success "已终止同步并清理状态。"
}

main() {
    # 1. 检查特殊模式 (Continue / Abort)
    if [[ "$MODE" == "continue" ]]; then
        handle_continue "$1"
        exit $?
    elif [[ "$MODE" == "abort" ]]; then
        handle_abort "$1"
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

    # 检查是否有未完成的同步
    if check_pending_sync "$target_repo"; then
        show_pending_sync_info "$target_repo"
        exit 1
    fi

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
        log_error "目标仓库处于同步中断状态。"
        log_detail "请先解决冲突后运行: $(basename "$0") --continue $target_repo"
        log_detail "或使用 --abort 终止: $(basename "$0") --abort $target_repo"
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
    # 不再使用 EXIT trap，改为手动清理

    # 分支解析逻辑
    cd "$source_repo" || exit 1
    local current_src_br=$(git rev-parse --abbrev-ref HEAD)
    local effective_src_br="${SOURCE_BRANCH:-$current_src_br}"
    
    # 如果指定了源分支且与当前分支不一致，切换过去
    if [[ -n "$SOURCE_BRANCH" && "$SOURCE_BRANCH" != "$current_src_br" ]]; then
        log_info "切换源仓库分支: $current_src_br -> $SOURCE_BRANCH"
        if ! git checkout "$SOURCE_BRANCH" --quiet 2>/dev/null; then
            log_error "无法切换到源分支: $SOURCE_BRANCH"
            exit 1
        fi
    fi
    
    # 如果范围包含 HEAD，替换为明确的分支名以确保稳定性
    if [[ "$rev_range" == *"HEAD"* ]]; then
        log_info "锁定源分支: $effective_src_br (解析自 HEAD)"
        rev_range="${rev_range/HEAD/$effective_src_br}"
    elif [[ "$rev_range" != *".."* ]]; then
        # 默认从该点到分支 HEAD
        rev_range="${rev_range}..${effective_src_br}"
    fi

    # 处理 --include-start 选项
    if $INCLUDE_START && [[ "$rev_range" == *".."* ]] && [[ "$rev_range" != *"^.."* ]]; then
        local start_commit="${rev_range%..*}"
        local end_commit="${rev_range#*..}"
        rev_range="${start_commit}^..${end_commit}"
        log_info "已包含起始 commit (转换为: $rev_range)"
    fi

    # 智能检测并提示
    if [[ "$rev_range" == *".."* ]] && [[ "$rev_range" != *"^.."* ]] && ! $INCLUDE_START; then
        local start_commit="${rev_range%..*}"
        log_warn "注意: 起始 commit ($start_commit) 不会被包含"
        log_detail "如需包含，请使用 --include-start 选项"
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
        rm -rf "$patch_dir"
        exit 0
    fi

    log_info "共发现 ${#patches[@]} 个待同步 Commit。"

    # 预览模式 (增强版)
    if $DRY_RUN; then
        echo -e "\n${CYAN}━━━ 待同步列表 ━━━${NC}"
        echo ""
        local idx=0
        for p in "${patches[@]}"; do
            ((idx++))
            local subj=$(grep "^Subject: " "$p" | head -1 | sed 's/Subject: \[PATCH.*\] //g')
            local file_count=$(get_patch_file_count "$p")
            local commit_hash=$(grep "^From " "$p" | head -1 | awk '{print substr($2,1,7)}')
            
            # 检查是否可能是空补丁
            local empty_hint=""
            if check_if_likely_empty "$p" "$source_repo" "$source_subdir"; then
                empty_hint="${YELLOW}(可能为空)${NC} "
            fi
            
            printf "${GRAY}%2d.${NC} ${CYAN}%s${NC} %s${GRAY}[%d 文件]${NC}\n" \
                "$idx" "$commit_hash" "$empty_hint" "$file_count"
            printf "    %s\n" "${subj:0:60}"
        done
        
        echo ""
        echo -e "${CYAN}━━━ 摘要 ━━━${NC}"
        echo -e "  Commit 范围: ${GRAY}$rev_range${NC}"
        echo -e "  同步路径: ${GRAY}$source_subdir${NC}"
        echo -e "  待同步数量: ${GREEN}${#patches[@]}${NC} 个 commit"
        
        if [[ "$rev_range" == *".."* ]] && [[ "$rev_range" != *"^.."* ]]; then
            echo -e "  ${YELLOW}提示: 起始 commit 未包含，如需包含请使用 --include-start${NC}"
        fi
        
        echo ""
        log_success "预览完成，未执行实际变更。"
        rm -rf "$patch_dir"
        exit 0
    fi

    # 保存状态以支持 --continue
    save_state "$target_repo" "$source_repo" "$source_subdir" "$rev_range" "$patch_dir"
    write_log "$target_repo" "开始同步: $source_subdir from $source_repo ($rev_range), ${#patches[@]} commits"

    # 5. 执行补丁应用
    cd "$target_repo" || exit 1
    local am_args=("--3way" "--committer-date-is-author-date")
    [[ -n "$TARGET_SUBDIR" ]] && am_args+=("--directory=$TARGET_SUBDIR")

    local count=0
    local skipped=0
    for p in "${patches[@]}"; do
        ((count++))
        local subj=$(grep "^Subject: " "$p" | head -1 | sed 's/Subject: \[PATCH.*\] //g')
        local commit_hash=$(grep "^From " "$p" | head -1 | awk '{print substr($2,1,7)}')
        
        # 交互模式
        if $INTERACTIVE; then
            echo ""
            echo -e "${CYAN}[$count/${#patches[@]}]${NC} $commit_hash - ${subj:0:50}"
            if ! ask_user "  是否同步此 commit?" "y"; then
                echo -e "  ${YELLOW}跳过${NC}"
                ((skipped++))
                update_progress "$target_repo" "$count"
                continue
            fi
        fi
        
        printf "[%d/%d] Applying: %s... " "$count" "${#patches[@]}" "${subj:0:40}"
        
        if git am "${am_args[@]}" "$p" > /dev/null 2>&1; then
            echo -e "${GREEN}完成${NC}"
            update_progress "$target_repo" "$count"
        else
            # 处理空补丁 (该 Commit 虽然在范围内，但没改目标子目录)
            if [[ -d .git/rebase-apply ]] && ! git status --porcelain | grep -q "^[UD]"; then
                echo -e "${YELLOW}空补丁${NC}"
                if $SKIP_EMPTY; then
                    log_detail "自动跳过 (--skip-empty)"
                    git am --skip > /dev/null 2>&1
                    update_progress "$target_repo" "$count"
                    continue
                elif ask_user "  --> 提交无实质内容变更，是否跳过?" "y"; then
                    git am --skip > /dev/null 2>&1
                    update_progress "$target_repo" "$count"
                    continue
                else
                    log_error "操作已由用户中止。"
                    rm -rf "$patch_dir"
                    exit 1
                fi
            else
                # 真正的合并冲突
                echo -e "${RED}失败${NC}"
                update_progress "$target_repo" "$((count - 1))"
                log_error "遇到合并冲突！请解决后运行:"
                log_detail "cd $target_repo && $(basename "$0") --continue"
                log_detail "或: $(basename "$0") --continue $target_repo"
                rm -rf "$patch_dir"
                exit 1
            fi
        fi
    done

    # 清理
    rm -rf "$patch_dir"
    cleanup_state "$target_repo"
    
    # 完成日志
    local applied=$((count - skipped))
    write_log "$target_repo" "同步完成: $applied commits applied, $skipped skipped"
    
    echo ""
    if [[ $skipped -gt 0 ]]; then
        log_success "同步完成！已应用 $applied 个 commit，跳过 $skipped 个。"
    else
        log_success "所有变更同步成功！"
    fi
}

main "$@"
