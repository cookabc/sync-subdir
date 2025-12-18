#!/bin/bash
#
# sync-subdir-changes.sh - 子目录变更同步工具
# 将源仓库中某个子目录的变更同步到独立的目标仓库
#

set -e

################################################################################
# 常量和颜色定义
################################################################################

readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly BLUE='\033[0;34m'
readonly NC='\033[0m'

################################################################################
# 工具函数
################################################################################

show_help() {
    cat << 'EOF'
用法: sync-subdir [选项] <源仓库> <子目录> <目标仓库> <起始commit>

将源仓库中某个子目录自指定 commit 以来的变更同步到独立的目标仓库。

参数:
    源仓库        源 Git 仓库路径
    子目录        源仓库中要同步的子目录名称
    目标仓库      目标 Git 仓库路径
    起始commit    起始 commit hash

选项:
    -b, --branch <分支>       源仓库分支 (默认: 当前分支)
    -t, --target-branch <分支> 目标仓库分支 (默认: 与源分支同名)
    -e, --end <commit>        结束 commit (默认: HEAD)

    -c, --create-branch       自动创建目标分支 (默认)
    --no-create-branch        禁止自动创建目标分支

    -i, --include-start       包含起始 commit 的变更 (默认)
    --no-include-start        不包含起始 commit 的变更

    -n, --no-merge            排除 merge 引入的变更
    --delete                  同步删除操作 (默认)
    --no-delete               不同步删除操作
    --stash                   自动 stash 目标仓库未提交变更

    -d, --dry-run             预览模式，不实际执行
    -v, --verbose             详细输出
    -y, --yes                 跳过确认，使用默认值
    -h, --help                显示帮助

交互模式:
    未通过参数指定时，脚本会询问:
    - 是否创建新分支、是否 stash、是否包含起始 commit
    - 是否排除 merge 变更、是否同步删除
    使用 -y 或 -d 跳过询问

示例:
    sync-subdir /repo/main submodule /repo/sub abc123
    sync-subdir -b feature/x -n /repo/main submodule /repo/sub abc123
EOF
}

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $1"; }

# 交互式询问 (返回: 0=yes, 1=no)
ask_user() {
    local question="$1" default="$2"

    # -y 或 dry-run 模式使用默认值
    if $YES || $DRY_RUN; then
        [[ "$default" == "y" ]] && return 0 || return 1
    fi

    local prompt="$question "
    [[ "$default" == "y" ]] && prompt+="[Y/n] " || prompt+="[y/N] "

    read -p "$prompt" -n 1 -r
    echo ""

    if [[ -z "$REPLY" ]]; then
        [[ "$default" == "y" ]] && return 0 || return 1
    fi
    [[ $REPLY =~ ^[Yy]$ ]] && return 0 || return 1
}

################################################################################
# 清理函数
################################################################################

RESTORE_TARGET_BRANCH=false
STASHED=false

cleanup() {
    local exit_code=$?

    # 恢复源仓库分支
    if [[ -n "$SOURCE_REPO" && -d "$SOURCE_REPO/.git" ]]; then
        cd "$SOURCE_REPO" 2>/dev/null || true
        if [[ -n "$SOURCE_ORIGINAL_BRANCH" && "$SOURCE_BRANCH" != "$SOURCE_ORIGINAL_BRANCH" ]]; then
            git checkout "$SOURCE_ORIGINAL_BRANCH" --quiet 2>/dev/null || true
        fi
    fi

    # 恢复目标仓库
    if $RESTORE_TARGET_BRANCH && [[ -n "$TARGET_REPO" && -d "$TARGET_REPO/.git" ]]; then
        cd "$TARGET_REPO" 2>/dev/null || true
        $STASHED && git stash pop --quiet 2>/dev/null || true
        if [[ -n "$TARGET_ORIGINAL_BRANCH" && "$TARGET_BRANCH" != "$TARGET_ORIGINAL_BRANCH" ]]; then
            git checkout "$TARGET_ORIGINAL_BRANCH" --quiet 2>/dev/null || true
        fi
    fi

    exit $exit_code
}

trap cleanup EXIT

################################################################################
# 默认值和选项解析
################################################################################

# 选项默认值
NO_MERGE=false
DRY_RUN=false
VERBOSE=false
YES=false
AUTO_STASH=false
CREATE_BRANCH=true
INCLUDE_START=true
SYNC_DELETE=true
SOURCE_BRANCH=""
TARGET_BRANCH=""
END_COMMIT=""

# 跟踪用户是否明确指定了选项
STASH_SPECIFIED=false
CREATE_BRANCH_SPECIFIED=false
INCLUDE_START_SPECIFIED=false
SYNC_DELETE_SPECIFIED=false
NO_MERGE_SPECIFIED=false

while [[ $# -gt 0 ]]; do
    case $1 in
        -b|--branch)         SOURCE_BRANCH="$2"; shift 2 ;;
        -t|--target-branch)  TARGET_BRANCH="$2"; shift 2 ;;
        -e|--end)            END_COMMIT="$2"; shift 2 ;;
        -c|--create-branch)  CREATE_BRANCH=true;  CREATE_BRANCH_SPECIFIED=true; shift ;;
        --no-create-branch)  CREATE_BRANCH=false; CREATE_BRANCH_SPECIFIED=true; shift ;;
        -i|--include-start)  INCLUDE_START=true;  INCLUDE_START_SPECIFIED=true; shift ;;
        --no-include-start)  INCLUDE_START=false; INCLUDE_START_SPECIFIED=true; shift ;;
        -n|--no-merge)       NO_MERGE=true; NO_MERGE_SPECIFIED=true; shift ;;
        --delete)            SYNC_DELETE=true;  SYNC_DELETE_SPECIFIED=true; shift ;;
        --no-delete)         SYNC_DELETE=false; SYNC_DELETE_SPECIFIED=true; shift ;;
        --stash)             AUTO_STASH=true; STASH_SPECIFIED=true; shift ;;
        -d|--dry-run)        DRY_RUN=true; shift ;;
        -v|--verbose)        VERBOSE=true; shift ;;
        -y|--yes)            YES=true; shift ;;
        -h|--help)           show_help; exit 0 ;;
        -*)                  log_error "未知选项: $1"; show_help; exit 1 ;;
        *)                   break ;;
    esac
done

# 检查必需参数
if [[ $# -lt 4 ]]; then
    log_error "参数不足"
    show_help
    exit 1
fi

SOURCE_REPO="$1"
SUBDIR="$2"
TARGET_REPO="$3"
START_COMMIT="$4"

################################################################################
# 验证和初始化
################################################################################

# 验证仓库路径
[[ ! -d "$SOURCE_REPO/.git" ]] && { log_error "无效的源仓库: $SOURCE_REPO"; exit 1; }
[[ ! -d "$TARGET_REPO/.git" ]] && { log_error "无效的目标仓库: $TARGET_REPO"; exit 1; }
[[ ! -d "$SOURCE_REPO/$SUBDIR" ]] && { log_error "子目录不存在: $SUBDIR"; exit 1; }

# 切换到源仓库
cd "$SOURCE_REPO"
SOURCE_ORIGINAL_BRANCH=$(git rev-parse --abbrev-ref HEAD)

# 切换源分支
if [[ -n "$SOURCE_BRANCH" ]]; then
    git rev-parse --verify "$SOURCE_BRANCH" > /dev/null 2>&1 || { log_error "源分支不存在: $SOURCE_BRANCH"; exit 1; }
    log_info "切换源仓库到分支: $SOURCE_BRANCH"
    git checkout "$SOURCE_BRANCH" --quiet
else
    SOURCE_BRANCH="$SOURCE_ORIGINAL_BRANCH"
fi

# 验证 commit
git rev-parse --verify "$START_COMMIT" > /dev/null 2>&1 || { log_error "无效的 commit: $START_COMMIT"; exit 1; }

# 切换到目标仓库
cd "$TARGET_REPO"
TARGET_ORIGINAL_BRANCH=$(git rev-parse --abbrev-ref HEAD)
[[ -z "$TARGET_BRANCH" ]] && TARGET_BRANCH="$SOURCE_BRANCH"

# 处理目标分支
if ! git rev-parse --verify "$TARGET_BRANCH" > /dev/null 2>&1; then
    if $CREATE_BRANCH_SPECIFIED; then
        if $CREATE_BRANCH; then
            log_info "创建目标分支: $TARGET_BRANCH"
            git checkout -b "$TARGET_BRANCH" --quiet
        else
            log_error "目标分支不存在: $TARGET_BRANCH"
            exit 1
        fi
    else
        log_warn "目标分支不存在: $TARGET_BRANCH"
        if ask_user "是否创建新分支?" "y"; then
            git checkout -b "$TARGET_BRANCH" --quiet
        else
            exit 1
        fi
    fi
elif [[ "$TARGET_BRANCH" != "$TARGET_ORIGINAL_BRANCH" ]]; then
    log_info "切换目标仓库到分支: $TARGET_BRANCH"
    git checkout "$TARGET_BRANCH" --quiet
fi

# 检查未提交变更
if ! git diff --quiet || ! git diff --cached --quiet; then
    if $STASH_SPECIFIED && $AUTO_STASH; then
        log_info "Stash 目标仓库变更..."
        git stash push -m "sync-subdir auto stash $(date +%Y%m%d-%H%M%S)" --quiet
        STASHED=true
    elif ! $STASH_SPECIFIED; then
        log_warn "目标仓库有未提交的变更"
        if ask_user "是否自动 stash?" "y"; then
            git stash push -m "sync-subdir auto stash $(date +%Y%m%d-%H%M%S)" --quiet
            STASHED=true
        else
            log_error "请先处理未提交的变更"
            exit 1
        fi
    else
        log_error "目标仓库有未提交的变更"
        exit 1
    fi
fi

# 切回源仓库
cd "$SOURCE_REPO"

# 设置结束 commit
if [[ -z "$END_COMMIT" ]]; then
    END_COMMIT="HEAD"
else
    git rev-parse --verify "$END_COMMIT" > /dev/null 2>&1 || { log_error "无效的结束 commit: $END_COMMIT"; exit 1; }
fi

################################################################################
# 显示配置信息
################################################################################

log_info "源仓库: $SOURCE_REPO ($SOURCE_BRANCH)"
log_info "目标仓库: $TARGET_REPO ($TARGET_BRANCH)"
log_info "子目录: $SUBDIR"
log_info "Commit 范围: $START_COMMIT...$END_COMMIT"
echo ""

# 询问是否包含起始 commit
if ! $INCLUDE_START_SPECIFIED && ! $YES && ! $DRY_RUN; then
    if ! ask_user "是否包含起始 commit 的变更?" "y"; then
        INCLUDE_START=false
    fi
fi

################################################################################
# 分析变更
################################################################################

# 设置 commit 范围
INCLUDE_ROOT_COMMIT=false
if $INCLUDE_START; then
    if git rev-parse --verify "${START_COMMIT}^" > /dev/null 2>&1; then
        COMMIT_RANGE="${START_COMMIT}^..${END_COMMIT}"
    else
        log_info "起始 commit 是首个 commit"
        COMMIT_RANGE="${START_COMMIT}..${END_COMMIT}"
        INCLUDE_ROOT_COMMIT=true
    fi
else
    COMMIT_RANGE="${START_COMMIT}..${END_COMMIT}"
fi

log_info "正在分析变更..."

# 获取变更文件列表
declare -a ALL_FILES FILES_TO_SYNC FILES_TO_RESTORE

while IFS= read -r file; do
    [[ -n "$file" ]] && ALL_FILES+=("$file")
done < <(git diff --name-only "$COMMIT_RANGE" -- "$SUBDIR/")

if $INCLUDE_ROOT_COMMIT; then
    while IFS= read -r file; do
        [[ -n "$file" ]] && ALL_FILES+=("$file")
    done < <(git diff-tree --no-commit-id --name-only -r "$START_COMMIT" -- "$SUBDIR/" 2>/dev/null || true)
    readarray -t ALL_FILES < <(printf '%s\n' "${ALL_FILES[@]}" | sort -u)
fi

if [[ ${#ALL_FILES[@]} -eq 0 ]]; then
    log_warn "没有发现任何变更"
    exit 0
fi

# 统计提交
TOTAL_COMMITS=$(git log --oneline "$COMMIT_RANGE" -- "$SUBDIR/" | wc -l | tr -d ' ')
MERGE_COMMITS=$(git log --oneline --merges "$COMMIT_RANGE" -- "$SUBDIR/" | wc -l | tr -d ' ')

log_info "提交数: $TOTAL_COMMITS (直接: $((TOTAL_COMMITS - MERGE_COMMITS)), Merge: $MERGE_COMMITS)"

# 询问是否排除 merge
if [[ $MERGE_COMMITS -gt 0 ]] && ! $NO_MERGE_SPECIFIED; then
    log_warn "检测到 $MERGE_COMMITS 个 merge 提交"
    if ask_user "是否排除 merge 引入的变更?" "y"; then
        NO_MERGE=true
    fi
fi

# 处理 merge 排除
DIRECT_FILES=""
if $NO_MERGE && [[ $MERGE_COMMITS -gt 0 ]]; then
    log_info "排除 merge 引入的变更"
    DIRECT_FILES=$(git log --name-only --no-merges --first-parent --format="" "$COMMIT_RANGE" -- "$SUBDIR/" | sort -u | grep -v '^$')

    has_excluded=false
    for file in "${ALL_FILES[@]}"; do
        if ! echo "$DIRECT_FILES" | grep -qx "$file"; then
            $has_excluded || { log_warn "以下文件将被排除:"; has_excluded=true; }
            echo "  - $file"
        fi
    done
    $has_excluded && echo ""
fi

# 分类文件
for file in "${ALL_FILES[@]}"; do
    if $NO_MERGE && [[ $MERGE_COMMITS -gt 0 ]]; then
        if echo "$DIRECT_FILES" | grep -qx "$file"; then
            FILES_TO_SYNC+=("$file")
        else
            FILES_TO_RESTORE+=("$file")
        fi
    else
        FILES_TO_SYNC+=("$file")
    fi
done

################################################################################
# 显示文件列表
################################################################################

log_info "将同步 ${#FILES_TO_SYNC[@]} 个文件:"

for file in "${FILES_TO_SYNC[@]}"; do
    if $VERBOSE; then
        STATUS=$(git diff --name-status "$COMMIT_RANGE" -- "$file" 2>/dev/null | cut -f1)
        case $STATUS in
            A) echo -e "  ${GREEN}[+]${NC} $file" ;;
            M) echo -e "  ${YELLOW}[~]${NC} $file" ;;
            D) echo -e "  ${RED}[-]${NC} $file" ;;
            *) echo "  $file" ;;
        esac
    else
        echo "  - $file"
    fi
done
echo ""

# Dry-run 模式退出
if $DRY_RUN; then
    log_warn "Dry-run 模式，不执行实际操作"
    RESTORE_TARGET_BRANCH=true
    exit 0
fi

################################################################################
# 执行同步
################################################################################

# 检测删除文件
DELETED_COUNT=0
for file in "${FILES_TO_SYNC[@]}"; do
    if [[ ! -f "$SOURCE_REPO/$file" && -f "$TARGET_REPO/${file#$SUBDIR/}" ]]; then
        ((DELETED_COUNT++))
    fi
done

if [[ $DELETED_COUNT -gt 0 ]] && ! $SYNC_DELETE_SPECIFIED; then
    log_warn "检测到 $DELETED_COUNT 个文件已删除"
    if ! ask_user "是否同步删除?" "y"; then
        SYNC_DELETE=false
    fi
fi

# 最终确认
if ! $YES; then
    if ! ask_user "是否执行同步?" "n"; then
        log_warn "操作已取消"
        RESTORE_TARGET_BRANCH=true
        exit 0
    fi
fi

log_info "正在同步..."

SYNCED=0 DELETED=0 FAILED=0
TOTAL=${#FILES_TO_SYNC[@]} CURRENT=0

for file in "${FILES_TO_SYNC[@]}"; do
    ((CURRENT++))
    RELATIVE="${file#$SUBDIR/}"
    DEST="$TARGET_REPO/$RELATIVE"

    $VERBOSE && printf "\r[%d/%d] %s...                    \r" "$CURRENT" "$TOTAL" "${RELATIVE:0:40}"

    # 处理删除
    if [[ ! -f "$SOURCE_REPO/$file" ]]; then
        if [[ -f "$DEST" ]] && $SYNC_DELETE; then
            rm "$DEST" && ((DELETED++)) || ((FAILED++))
        fi
        continue
    fi

    # 创建目录并复制
    mkdir -p "$(dirname "$DEST")"
    cp "$SOURCE_REPO/$file" "$DEST" && ((SYNCED++)) || ((FAILED++))
done
$VERBOSE && echo ""

# 恢复 merge 排除的文件
if $NO_MERGE && [[ ${#FILES_TO_RESTORE[@]} -gt 0 ]]; then
    log_info "恢复被排除的文件..."
    cd "$TARGET_REPO"
    for file in "${FILES_TO_RESTORE[@]}"; do
        RELATIVE="${file#$SUBDIR/}"
        git ls-files --error-unmatch "$RELATIVE" > /dev/null 2>&1 && \
            git checkout HEAD -- "$RELATIVE" 2>/dev/null || true
    done
fi

################################################################################
# 完成
################################################################################

echo ""
log_success "同步完成！"
log_info "同步: $SYNCED  删除: $DELETED  失败: $FAILED"

echo ""
log_info "目标仓库状态:"
cd "$TARGET_REPO"
git status --short

if $STASHED; then
    echo ""
    log_warn "之前 stash 了变更，请手动执行 'git stash pop'"
fi
