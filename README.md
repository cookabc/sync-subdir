# Tools

开发工具集合

## sync-subdir.sh

子目录变更同步工具 - 将源仓库中某个子目录的变更同步到独立的目标仓库。

### 使用场景

当你从一个 monorepo 中拆分出子模块，但原仓库仍在继续开发时，可以用这个工具将原仓库子目录的变更同步到新的独立仓库。

### 安装

```bash
chmod +x sync-subdir.sh
# 可选: 添加到 PATH
ln -s /path/to/sync-subdir.sh /usr/local/bin/sync-subdir
```

### 基本用法

```bash
sync-subdir.sh <源仓库> <子目录> <目标仓库> <起始commit>
```

### 参数说明

| 参数 | 说明 |
|------|------|
| 源仓库 | 源 Git 仓库路径 |
| 子目录 | 源仓库中要同步的子目录名称 |
| 目标仓库 | 目标 Git 仓库路径 |
| 起始commit | 从哪个 commit 开始同步 |

### 选项

| 选项 | 说明 | 默认值 |
|------|------|--------|
| `-b, --branch` | 源仓库分支 | 当前分支 |
| `-t, --target-branch` | 目标仓库分支 | 与源分支同名 |
| `-e, --end` | 结束 commit | HEAD |
| `-c, --create-branch` | 自动创建目标分支 | 是 |
| `--no-create-branch` | 禁止自动创建 | - |
| `-i, --include-start` | 包含起始 commit | 是 |
| `--no-include-start` | 不包含起始 commit | - |
| `-n, --no-merge` | 排除 merge 引入的变更 | 否 |
| `--delete` | 同步删除操作 | 是 |
| `--no-delete` | 不同步删除 | - |
| `--stash` | 自动 stash 目标仓库变更 | 否 |
| `-d, --dry-run` | 预览模式 | - |
| `-v, --verbose` | 详细输出 | - |
| `-y, --yes` | 跳过确认 | - |

### 交互模式

未通过参数明确指定时，脚本会交互式询问：

- 目标分支不存在 → 是否创建？
- 目标仓库有未提交变更 → 是否 stash？
- 是否包含起始 commit？
- 检测到 merge → 是否排除 merge 引入的变更？
- 检测到删除 → 是否同步删除？

使用 `-y` 跳过询问并使用默认值。

### 示例

```bash
# 基本使用
sync-subdir.sh ~/repos/monorepo submodule ~/repos/submodule abc123

# 指定分支
sync-subdir.sh -b feature/x ~/repos/monorepo submodule ~/repos/submodule abc123

# 排除 merge 提交，详细输出
sync-subdir.sh -n -v ~/repos/monorepo submodule ~/repos/submodule abc123

# 预览模式
sync-subdir.sh -d ~/repos/monorepo submodule ~/repos/submodule abc123

# 非交互式执行
sync-subdir.sh -y ~/repos/monorepo submodule ~/repos/submodule abc123
```

### 工作流程

1. 验证仓库和分支
2. 检查目标仓库状态（未提交变更处理）
3. 分析 commit 范围内的文件变更
4. 识别 merge 引入的变更（可选排除）
5. 复制/删除文件到目标仓库
6. 显示目标仓库状态

### 注意事项

- 同步完成后需手动 commit 目标仓库的变更
- 如果使用了 stash，同步后需手动 `git stash pop`
- 脚本会在退出时自动恢复源仓库的原分支

