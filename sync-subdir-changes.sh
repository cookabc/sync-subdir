#!/bin/bash

# sync-subdir-changes.sh
# 将源仓库中某个子目录的变更同步到独立的目标仓库
# 支持排除 merge 提交引入的变更

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 打印帮助信息
show_help() {
    cat << EOF
用法: $(basename "$0") [选项] <源仓库> <子目录> <目标仓库> <起始commit>

将源仓库中某个子目录自指定 commit 以来的变更同步到独立的目标仓库。

参数:
    源仓库        源 Git 仓库路径
    子目录        源仓库中要同步的子目录名称
    目标仓库      目标 Git 仓库路径
    起始commit    起始 commit hash

选项:
    -b, --branch <分支>  指定源仓库的分支 (默认: 当前分支)
    -t, --target-branch <分支>  指定目标仓库的分支 (默认: 当前分支)
    -c, --create-branch  如果目标分支不存在则自动创建
    -i, --include-start  包含起始 commit 的变更 (默认不包含)
    -n, --no-merge       排除通过 merge 引入的变更，只同步直接提交
    -d, --dry-run        仅显示将要进行的操作，不实际执行
    -v, --verbose        显示详细输出
    -y, --yes            跳过确认提示，直接执行
    -h, --help           显示此帮助信息

示例:
    $(basename "$0") /path/to/funding funding-common /path/to/funding-common abc123
    $(basename "$0") -n /path/to/funding funding-common /path/to/funding-common abc123
    $(basename "$0") -b feature/xxx -t feature/xxx -c /path/to/funding funding-common /path/to/funding-common abc123

EOF
}

# 日志函数
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# 默认选项
NO_MERGE=false
DRY_RUN=false
VERBOSE=false
YES=false
CREATE_BRANCH=false
INCLUDE_START=false
SOURCE_BRANCH=""
TARGET_BRANCH=""

# 解析选项
while [[ $# -gt 0 ]]; do
    case $1 in
        -b|--branch)
            SOURCE_BRANCH="$2"
            shift 2
            ;;
        -t|--target-branch)
            TARGET_BRANCH="$2"
            shift 2
            ;;
        -c|--create-branch)
            CREATE_BRANCH=true
            shift
            ;;
        -i|--include-start)
            INCLUDE_START=true
            shift
            ;;
        -n|--no-merge)
            NO_MERGE=true
            shift
            ;;
        -d|--dry-run)
            DRY_RUN=true
            shift
            ;;
        -v|--verbose)
            VERBOSE=true
            shift
            ;;
        -y|--yes)
            YES=true
            shift
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        -*)
            log_error "未知选项: $1"
            show_help
            exit 1
            ;;
        *)
            break
            ;;
    esac
done

# 检查参数数量
if [[ $# -lt 4 ]]; then
    log_error "参数不足"
    show_help
    exit 1
fi

SOURCE_REPO="$1"
SUBDIR="$2"
TARGET_REPO="$3"
START_COMMIT="$4"

# 验证路径
if [[ ! -d "$SOURCE_REPO/.git" ]]; then
    log_error "源仓库不是有效的 Git 仓库: $SOURCE_REPO"
    exit 1
fi

if [[ ! -d "$TARGET_REPO/.git" ]]; then
    log_error "目标仓库不是有效的 Git 仓库: $TARGET_REPO"
    exit 1
fi

if [[ ! -d "$SOURCE_REPO/$SUBDIR" ]]; then
    log_error "源仓库中不存在子目录: $SUBDIR"
    exit 1
fi

# 切换到源仓库
cd "$SOURCE_REPO"

# 保存源仓库当前分支
SOURCE_ORIGINAL_BRANCH=$(git rev-parse --abbrev-ref HEAD)

# 如果指定了源分支，切换到该分支
if [[ -n "$SOURCE_BRANCH" ]]; then
    if ! git rev-parse --verify "$SOURCE_BRANCH" > /dev/null 2>&1; then
        log_error "源仓库中不存在分支: $SOURCE_BRANCH"
        exit 1
    fi
    log_info "切换源仓库到分支: $SOURCE_BRANCH"
    git checkout "$SOURCE_BRANCH" --quiet
else
    SOURCE_BRANCH="$SOURCE_ORIGINAL_BRANCH"
fi

# 验证 commit
if ! git rev-parse --verify "$START_COMMIT" > /dev/null 2>&1; then
    log_error "无效的 commit: $START_COMMIT"
    # 恢复原分支
    [[ "$SOURCE_BRANCH" != "$SOURCE_ORIGINAL_BRANCH" ]] && git checkout "$SOURCE_ORIGINAL_BRANCH" --quiet
    exit 1
fi

# 切换到目标仓库检查分支
cd "$TARGET_REPO"
TARGET_ORIGINAL_BRANCH=$(git rev-parse --abbrev-ref HEAD)

if [[ -n "$TARGET_BRANCH" ]]; then
    if ! git rev-parse --verify "$TARGET_BRANCH" > /dev/null 2>&1; then
        if $CREATE_BRANCH; then
            log_info "目标分支不存在，正在创建: $TARGET_BRANCH"
            git checkout -b "$TARGET_BRANCH" --quiet
        else
            log_error "目标仓库中不存在分支: $TARGET_BRANCH"
            log_info "提示: 使用 -c 选项可自动创建分支"
            # 恢复源仓库原分支
            cd "$SOURCE_REPO"
            [[ "$SOURCE_BRANCH" != "$SOURCE_ORIGINAL_BRANCH" ]] && git checkout "$SOURCE_ORIGINAL_BRANCH" --quiet
            exit 1
        fi
    else
        log_info "切换目标仓库到分支: $TARGET_BRANCH"
        git checkout "$TARGET_BRANCH" --quiet
    fi
else
    TARGET_BRANCH="$TARGET_ORIGINAL_BRANCH"
fi

# 切回源仓库进行后续操作
cd "$SOURCE_REPO"

log_info "源仓库: $SOURCE_REPO"
log_info "源分支: $SOURCE_BRANCH"
log_info "子目录: $SUBDIR"
log_info "目标仓库: $TARGET_REPO"
log_info "目标分支: $TARGET_BRANCH"
log_info "起始 commit: $START_COMMIT"
log_info "包含起始 commit: $INCLUDE_START"
log_info "排除 merge: $NO_MERGE"
echo ""

# 设置 commit 范围
if $INCLUDE_START; then
    # 包含起始 commit: 使用 commit^ 作为起点
    COMMIT_RANGE="${START_COMMIT}^..HEAD"
else
    # 不包含起始 commit
    COMMIT_RANGE="${START_COMMIT}..HEAD"
fi

# 获取所有变更的文件
log_info "正在分析变更..."

ALL_CHANGED_FILES=$(git diff --name-only "$COMMIT_RANGE" -- "$SUBDIR/")

if [[ -z "$ALL_CHANGED_FILES" ]]; then
    log_warn "没有发现任何变更"
    exit 0
fi

# 统计提交
TOTAL_COMMITS=$(git log --oneline "$COMMIT_RANGE" -- "$SUBDIR/" | wc -l | tr -d ' ')
MERGE_COMMITS=$(git log --oneline --merges "$COMMIT_RANGE" -- "$SUBDIR/" | wc -l | tr -d ' ')
DIRECT_COMMITS=$((TOTAL_COMMITS - MERGE_COMMITS))

log_info "总提交数: $TOTAL_COMMITS (直接提交: $DIRECT_COMMITS, Merge: $MERGE_COMMITS)"

if $NO_MERGE && [[ $MERGE_COMMITS -gt 0 ]]; then
    log_info "检测到 $MERGE_COMMITS 个 merge 提交，将排除其引入的变更"
    
    # 获取通过 merge 引入的提交
    DIRECT_COMMIT_HASHES=$(git log --oneline --no-merges --first-parent "$COMMIT_RANGE" -- "$SUBDIR/" | cut -d' ' -f1)
    ALL_COMMIT_HASHES=$(git log --oneline --no-merges "$COMMIT_RANGE" -- "$SUBDIR/" | cut -d' ' -f1)
    
    # 找出通过 merge 引入的提交
    MERGE_INTRODUCED_COMMITS=""
    for commit in $ALL_COMMIT_HASHES; do
        if ! echo "$DIRECT_COMMIT_HASHES" | grep -q "^$commit$"; then
            MERGE_INTRODUCED_COMMITS="$MERGE_INTRODUCED_COMMITS $commit"
        fi
    done
    
    # 找出只通过 merge 引入变更的文件
    MERGE_ONLY_FILES=""
    for file in $ALL_CHANGED_FILES; do
        # 检查这个文件是否在 first-parent 提交中有变更
        FIRST_PARENT_CHANGES=$(git log --oneline --no-merges --first-parent "$COMMIT_RANGE" -- "$file" | wc -l | tr -d ' ')
        if [[ $FIRST_PARENT_CHANGES -eq 0 ]]; then
            MERGE_ONLY_FILES="$MERGE_ONLY_FILES $file"
        fi
    done
    
    if [[ -n "$MERGE_ONLY_FILES" ]]; then
        log_warn "以下文件的变更仅来自 merge，将被排除:"
        for file in $MERGE_ONLY_FILES; do
            echo "  - $file"
        done
        echo ""
    fi
fi

# 确定要同步的文件
FILES_TO_SYNC=""
FILES_TO_RESTORE=""

for file in $ALL_CHANGED_FILES; do
    if $NO_MERGE; then
        FIRST_PARENT_CHANGES=$(git log --oneline --no-merges --first-parent "$COMMIT_RANGE" -- "$file" | wc -l | tr -d ' ')
        if [[ $FIRST_PARENT_CHANGES -gt 0 ]]; then
            FILES_TO_SYNC="$FILES_TO_SYNC $file"
        else
            FILES_TO_RESTORE="$FILES_TO_RESTORE $file"
        fi
    else
        FILES_TO_SYNC="$FILES_TO_SYNC $file"
    fi
done

# 显示要同步的文件
SYNC_COUNT=$(echo $FILES_TO_SYNC | wc -w | tr -d ' ')
log_info "将同步 $SYNC_COUNT 个文件:"

if $VERBOSE; then
    for file in $FILES_TO_SYNC; do
        # 检查是新增还是修改
        STATUS=$(git diff --name-status "$COMMIT_RANGE" -- "$file" | cut -f1)
        case $STATUS in
            A) echo -e "  ${GREEN}[新增]${NC} $file" ;;
            M) echo -e "  ${YELLOW}[修改]${NC} $file" ;;
            D) echo -e "  ${RED}[删除]${NC} $file" ;;
            *) echo "  $file" ;;
        esac
    done
else
    for file in $FILES_TO_SYNC; do
        echo "  - $file"
    done
fi

echo ""

# Dry run 模式
if $DRY_RUN; then
    log_warn "Dry-run 模式，不执行实际操作"
    # 恢复原分支
    cd "$SOURCE_REPO"
    [[ "$SOURCE_BRANCH" != "$SOURCE_ORIGINAL_BRANCH" ]] && git checkout "$SOURCE_ORIGINAL_BRANCH" --quiet
    cd "$TARGET_REPO"
    [[ "$TARGET_BRANCH" != "$TARGET_ORIGINAL_BRANCH" ]] && git checkout "$TARGET_ORIGINAL_BRANCH" --quiet
    exit 0
fi

# 确认执行
if ! $YES; then
    read -p "是否继续? [y/N] " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        log_warn "操作已取消"
        # 恢复原分支
        cd "$SOURCE_REPO"
        [[ "$SOURCE_BRANCH" != "$SOURCE_ORIGINAL_BRANCH" ]] && git checkout "$SOURCE_ORIGINAL_BRANCH" --quiet
        cd "$TARGET_REPO"
        [[ "$TARGET_BRANCH" != "$TARGET_ORIGINAL_BRANCH" ]] && git checkout "$TARGET_ORIGINAL_BRANCH" --quiet
        exit 0
    fi
fi

# 执行同步
log_info "正在同步文件..."

SYNCED=0
FAILED=0

for file in $FILES_TO_SYNC; do
    # 计算目标路径（去掉子目录前缀）
    RELATIVE_PATH="${file#$SUBDIR/}"
    DEST_FILE="$TARGET_REPO/$RELATIVE_PATH"
    
    # 检查文件是否被删除
    if [[ ! -f "$SOURCE_REPO/$file" ]]; then
        if [[ -f "$DEST_FILE" ]]; then
            log_warn "文件已在源仓库删除，跳过: $file"
        fi
        continue
    fi
    
    # 创建目标目录
    DEST_DIR=$(dirname "$DEST_FILE")
    if [[ ! -d "$DEST_DIR" ]]; then
        mkdir -p "$DEST_DIR"
        $VERBOSE && log_info "创建目录: $DEST_DIR"
    fi
    
    # 复制文件
    if cp "$SOURCE_REPO/$file" "$DEST_FILE"; then
        $VERBOSE && log_success "已复制: $file"
        ((SYNCED++))
    else
        log_error "复制失败: $file"
        ((FAILED++))
    fi
done

# 恢复只通过 merge 变更的文件
if $NO_MERGE && [[ -n "$FILES_TO_RESTORE" ]]; then
    log_info "正在恢复被 merge 修改的文件..."
    cd "$TARGET_REPO"
    
    for file in $FILES_TO_RESTORE; do
        RELATIVE_PATH="${file#$SUBDIR/}"
        if git ls-files --error-unmatch "$RELATIVE_PATH" > /dev/null 2>&1; then
            if git checkout HEAD -- "$RELATIVE_PATH" 2>/dev/null; then
                $VERBOSE && log_success "已恢复: $RELATIVE_PATH"
            fi
        fi
    done
fi

echo ""
log_success "同步完成！"
log_info "已同步: $SYNCED 个文件"
[[ $FAILED -gt 0 ]] && log_error "失败: $FAILED 个文件"

# 显示目标仓库状态
echo ""
log_info "目标仓库变更状态:"
cd "$TARGET_REPO"
git status --short

# 恢复源仓库原分支
cd "$SOURCE_REPO"
if [[ "$SOURCE_BRANCH" != "$SOURCE_ORIGINAL_BRANCH" ]]; then
    log_info "恢复源仓库到原分支: $SOURCE_ORIGINAL_BRANCH"
    git checkout "$SOURCE_ORIGINAL_BRANCH" --quiet
fi

