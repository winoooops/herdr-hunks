# herdr-hunks

[English](README.md) · **简体中文** · [日本語](README.ja.md)

适用于 [Herdr](https://herdr.dev) 的只读 git 差异块查看器，提供变更文件列表、
统一/并排差异视图、可点击的导航工具栏和实时刷新。
第一阶段不支持暂存、取消暂存、丢弃修改、添加评论或向代理派发任务。

## 安装

从 [GitHub](https://github.com/winoooops/herdr-hunks) 安装：

```sh
herdr plugin install winoooops/herdr-hunks
```

分支版本用户应使用自己的 `vimeflow` 可执行文件运行同样的命令，因为插件注册表相互独立。
安装会下载与检出版本对应的发行版二进制文件并校验 SHA256，支持 macOS 和 Linux 的 x86_64 与 arm64。
资源缺失、下载失败或校验不匹配时，会回退到 `cargo build --release` 从源码构建，这需要 Rust 1.88 或更新版本。

## 命令

通过宿主调用操作，由宿主提供插件 ID 和窗格上下文：

```sh
herdr plugin action invoke open --plugin winoooops.hunks
herdr plugin action invoke open-split --plugin winoooops.hunks
herdr plugin action invoke update --plugin winoooops.hunks
```

| 操作 | 行为 |
| --- | --- |
| `open` | 为发起窗格的工作目录打开并聚焦弹出对话框，默认宽高均为 80%。宿主必须处于普通工作区视图。 |
| `open-split` | 在右侧打开分割窗格；若会话、发起窗格、仓库、标题和 cwd 仍匹配，则聚焦已有查看器。 |
| `update` | 对 GitHub 安装，当最新发行标签比当前运行版本更新时安装它，否则提示已是最新；拒绝本地链接或未知来源。 |

独立运行：`herdr-hunks [PATH]` 或 `herdr-hunks tui [PATH]`；PATH 默认为当前目录。
标准输入和标准输出都必须连接到终端。`--version` 输出 `herdr-hunks 0.0.3`。
可通过 `herdr plugin log list --plugin winoooops.hunks --limit 1` 查看操作诊断信息。

## 快捷键绑定

在宿主配置中添加：

```toml
[[keys.command]]
key = "prefix+d"
type = "plugin_action"
command = "winoooops.hunks.open"
description = "Open the hunk viewer"
```

要改为绑定分割窗格操作，将 `command` 改为 `"winoooops.hunks.open-split"`。
不会自动安装快捷键绑定。

## 按键

| 按键 | 操作 |
| --- | --- |
| `j` / `k`、Down / Up | 下一行 / 上一行 |
| Ctrl+D / Ctrl+U、PageDown / PageUp | 向下 / 向上半页 |
| `[` / `]` | 上一个 / 下一个差异块 |
| `n` / `p` | 下一个 / 上一个文件 |
| `h` / `l`、Left / Right | 删除侧 / 新增侧 |
| `t` | 切换统一 / 并排视图；并排视图至少需要 100 列 |
| `e` / `E` | 切换 / 固定文件面板 |
| `r` | 刷新 |
| `b` | 切换范围：worktree <-> branch |
| `B` | 选择比较基准 |
| `M` | 将当前提交标记为已审阅 |
| `g` / `G`、Home / End | 第一行 / 最后一行 |
| `H` / `L` | 向左 / 向右滚动八个显示单元 |
| `m` | 切换鼠标捕获 |
| `?` | 打开按键帮助；用 `j` / `k` 或 Down / Up 滚动 |
| `q` | 退出；若按键帮助已打开，则先关闭帮助 |
| Esc | 关闭按键帮助或基准选择器；否则关闭弹出查看器 |
| Ctrl+C | 立即退出，包括在按键帮助或基准选择器中 |

方向键、PageDown、PageUp、Home 和 End 的别名仅在不带修饰键时生效。
第一阶段保留 `s d D i I u U x v y Y @ c /`，不绑定任何操作。

## 鼠标

工具栏控件是带左右留白、粗体和反色的按钮。禁用控件变暗且不可点击：
文件少于两个时的文件切换按钮、已加载区块少于两个时的区块切换按钮，
以及所请求模式为统一视图且宽度不足 100 列时的视图按钮。未解析到基准时，范围按钮也会禁用。
悬停按钮会高亮，并在页脚显示说明和按键；悬停文件行会加粗。
移开后恢复通常的按键提示。可点击按钮、文件行和差异行。滚轮每次滚动三行。
按 `m` 禁用捕获，恢复终端原生文本选择；再次按下可启用捕获。禁用捕获会清除悬停状态。
帮助打开时，滚轮用于滚动帮助内容。在基准选择器中，点击一行即可选择，滚轮用于移动光标。

## 配置

`config.toml` 位于 `$HERDR_PLUGIN_CONFIG_DIR`，否则位于
`${XDG_CONFIG_HOME:-$HOME/.config}/herdr-hunks`：

```toml
[view]
mode = "auto"      # auto, unified, split
files = "auto"     # auto, pinned, hidden
scope = "worktree" # worktree, branch

[input]
mouse = true

[base]
ref = "main"       # 任意修订；参见“分支范围”

[popup]
width = "80%"
height = "80%"
```

弹窗尺寸接受大于等于 20 的整数（包括边框的总单元格数），或 "20%" 到 "100%" 的百分比。
无效尺寸会逐项回退为 "80%"，`open` 操作在标准错误中报告问题。
配置文件缺失时使用默认值；文件不可读或格式错误时会报告问题并使用默认值。

根据初始宽度，auto 视图模式在达到 120 列时选择并排视图，auto 文件面板在达到 100 列时固定。
选择并排视图后，宽度不足 100 列会临时改为统一视图，恢复宽度后再切回。
尺寸小于 40×10 时仅显示尺寸提示。
无效的 view/input/base 设置会逐项回退并显示提示；诊断信息写入 `$HERDR_PLUGIN_STATE_DIR` 下的
`config-problems.log`，否则使用 `${XDG_STATE_HOME:-$HOME/.local/state}/herdr-hunks`。

目录设置必须是绝对路径；空值或相对路径会继续尝试下一个绝对路径设置。
没有可用状态目录时，仍可打开分割窗格，但不复用已有窗格，并报告原因。
不会根据仓库位置推导状态目录。

## 分支范围

查看器有两种范围。`worktree`（默认）列出 `git status` 中的内容：
相对于 `HEAD` 的已暂存、未暂存和未跟踪更改。`branch` 列出当前分支相对于基准的全部更改：
将工作树与 `merge-base(HEAD, base)` 比较，无论是否已提交，每个路径显示一行，另加未跟踪文件。
按 `b` 切换（工具栏按钮显示 `worktree` 或 `vs <base>`）；
将 `prefix+f` 绑定到 `open-split` 后，依次按 `prefix`、`f`、`b` 即可进入。
在分支上删除后又以未跟踪文件重建的路径会显示为两行。

基准按以下顺序解析，采用第一个指向提交的值：

1. 在此工作树中用 `B` 选择的基准（保存在状态目录的 `bases.json` 中）；
2. `config.toml` 中的 `[base] ref`；
3. `refs/heads/main`；
4. 远程仓库的默认分支（`refs/remotes/origin/HEAD`）；
5. `refs/heads/master`。

如果都无法解析，按 `b` 会显示 `no base branch: set [base] ref or press B`。

`B` 打开“Compare against”：输入文字可筛选本地分支、远程分支和标签
（按新到旧排列，最多 200 条；其余项可直接输入），也可输入任意修订，例如 `origin/main` 或 `HEAD~3`。
`Enter` 选择，`Esc` 取消，第一行 `default (...)` 用于清除已选基准。
从列表选择的引用会以完整限定名传给 git；自由输入和 `[base] ref` 按原文传入，
因此存在同名标签时应写成 `refs/heads/x`。选择的基准按工作树保存；
状态目录不可用时，仅在当前会话内有效。

分支范围与查看器的其他部分一样为只读，只在运行的 git 命令中增加 `merge-base` 和 `for-each-ref`。
每次刷新都会重新检查基准，因此基准移动后（例如合并到 `main` 或执行 fetch），
会在一个轮询间隔内反映出来。

## 审阅标记

在分支范围中，按 `M` 为当前工作树记录已审阅到的提交。
之后的提交涉及的行会在文件面板中显示 `●`，再次按 `M` 即可清除这些标记。
圆点反映提交，而不是尚未提交的编辑：未提交的更改不会产生圆点，
因此 `M` 总会清除所有圆点，编辑内容仍照常显示在该行的差异中。
未跟踪的行不会带有圆点。

标记保存在状态目录中，与 `bases.json` 相邻的 `marks.json` 文件里，
每个工作树保存一个提交。因此，关闭查看器后仍会保留，
同一工作树的第二个查看器也会共享它。标记不会改变比较基准：
如果标记的提交被历史改写移出当前分支，或对象被清理，所有已跟踪的行都会被标记并显示提示，
而文件列表、差异和基准仍照常工作。

`B` 将标记作为可选基准，例如 `reviewed (a1b2c3d · 12 min ago)`；
选择后，列表会缩小到工作树与该标记之间的差异。
同一选择器还会在可解析时提供 `upstream`、`last commit` 和 `last 3 commits`，
常用基准无需再手动输入。

## 运行要求

- macOS 或 Linux，标准输入和标准输出连接到终端。
- PATH 中提供 Git 2.31 或更新版本。
- 插件操作需要 Herdr 0.8.0 或更新版本；独立运行不需要宿主。
- 本地构建或安装回退构建需要 Rust 1.88 或更新版本。

## 已知限制

本阶段为只读，尚未实现暂存和评论。
[PORT-SURFACE.md 列出了 K1–K7](PORT-SURFACE.md#known-defects)：K1–K4 是冻结代码中修改路径的缺陷，
第一阶段不会触发；K5 可能隐藏先暂存修改或新增、再删除文件时的暂存部分；
K6 会使已被新请求取代的慢速差异请求累积 git 进程。
在 worktree 范围中，若已暂存行的路径由文件变成目录，会把目录的补丁解析为该行自己的补丁
（PORT-SURFACE.md 中的 K7）；branch 范围不受影响。
不是合法 UTF-8 的路径会以 `\u{fffd}` 代替无法解码的字节显示；传给 git 的是替换后的名字，所以其差异为空
（或者是恰好叫这个名字的文件的差异）；仅在这些字节上不同的路径在 branch 范围合并为一行，
在 worktree 范围则只有同类行（staged、unstaged 或未跟踪）才会合并。
差异视图最多保留 200,000 行，超出后显示截断提示行。
`GIT_NO_LAZY_FETCH` 从 Git 2.45 起生效；更早的 Git 在读取部分克隆时可能下载缺失对象。
尚不支持语法高亮或展开上下文。

## 路线图

P1：只读查看。P2：暂存、取消暂存、丢弃操作及缺陷修复。
P3：评论和代理任务派发。P4：回复与讨论串。P5：委托审查。
参见[设计规范](docs/superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md)。

## 本地开发

在当前检出目录中运行：

```sh
cargo build --release
herdr plugin link "$PWD"
herdr plugin action invoke open --plugin winoooops.hunks
```

`plugin link` 会跳过 `[[build]]`，因此需要先构建。分支版本用户请替换为自己的 `vimeflow` 可执行文件。
使用 `cargo build --no-default-features` 可构建不含查看器的独立操作程序。

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -- --test-threads=1
cargo check --no-default-features
scripts/port-check.sh /path/to/vimeflow
sh scripts/port-check-selftest.sh /path/to/vimeflow
```

测试会在 HOME 下创建测试数据；该目录必须可写且位于 git 检出目录之外。
参考检出目录必须包含提交 `91e45b1c`，检查过程只读访问该目录。
CI 会尝试从 `winoooops/vimeflow` 检出该固定提交；若仓库为私有，请配置具有读取权限的
`VIMEFLOW_READ_TOKEN` Actions secret。无法检出时，CI 会显示提示并跳过两项移植检查；
仍可使用上面的命令在本地运行。

推送 `v*` 标签会触发发布工作流：构建四个目标，并要求标签与 Cargo.toml 的版本一致、`docs/acceptance-p1.md` 为 PASS。
请保持 Cargo.toml、Cargo.lock 和 herdr-plugin.toml 的版本同步。
`HERDR_HUNKS_RELEASE_BASE=file:///absolute/fixture` 可让分发检查使用本地资源，不访问 GitHub。

## 许可证

[Apache-2.0](LICENSE)。包含从 vimeflow（MIT，声明见
[third-party/vimeflow-LICENSE](third-party/vimeflow-LICENSE)）和 herdr-agent-watcher
（Apache-2.0）移植的代码；来源和适配说明见 [NOTICE](NOTICE) 与
[PORT-SURFACE.md](PORT-SURFACE.md)。
