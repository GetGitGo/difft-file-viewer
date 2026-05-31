# difft-file-viewer

[English](README.md) | [简体中文](README.zh-CN.md)

基于 [difftastic](https://github.com/wilfred/difftastic) 的 Slint GUI，用于对比 2 个文件，或 Apply 2 个文件到第 3 个文件。

Viewer 可对 C/C++ 输入可选执行 `clang-format`，再子进程调用 `difft`，读取 `--display json` 输出，以双栏或三栏展示 diff。

## 基本机制

```
                    ┌─ 可选：clang-format 副本 ─┐
                    │  (cwd 下 .clangformat)    │
                    ▼                          │
┌─────────────┐     subprocess      ┌──────────┐  │
│ difft-file- │  DFT_UNSTABLE=yes   │  difft   │  │
│ viewer      │ ──────────────────► │  (CLI)   │  │
└─────────────┘  --display json     └──────────┘  │
        │                                      │    │
        │         stdout: JSON（单个 object）   │    │
        └──────────────────────────────────────┘    │
                                                    │
  C/C++ 输入路径 ◄──────────────────────────────────┘
```

1. **前处理（可选）：** 对每个已存在的 C/C++ **diff 输入**（`file-a` / `file-b`），若**当前工作目录**下有非空 **`.clangformat`** 且 `PATH` 中有 `clang-format`，则在 diff 前生成格式化**副本**供 `difft` 使用，**不修改磁盘上的原文件**。详见 [C/C++ 格式化](#c-c-格式化-clang-format)。
2. **Diff：** 仅对 `file-a`、`file-b` 调用（`file-c` 不参与 difft）：

   ```text
   difft --display json --byte-limit 32000000 --context 999999 <file-a> <file-b>
   ```

3. 解析 JSON 对齐行（`lhs_text`、`rhs_text`、novel 标记、语法 span/块等）。
4. Slint 展示：
   - **两个参数：** **A | B** 双栏（A 侧红 = 删除/变更，B 侧绿 = 新增/变更）。
   - **三个参数：** **A | C | B** — diff 仍为 A vs B；**C** 按行号显示 `file-c`（不存在则创建）。可在 A/B 选中语法块 **Apply** 写入 C。

成功时状态区隐藏；错误、`clang-format` 警告、diff fallback 等显示在紫色信息区。

## 环境要求

| 组件 | 说明 |
|------|------|
| Rust | 1.85+（见 `Cargo.toml` 中 `rust-version`） |
| `difft` | 建议与本 crate 同版本（当前 **0.70.0**） |
| `clang-format` | 可选 — 仅用于 [C/C++ 前处理](#c-c-格式化-clang-format) |
| 系统 | macOS、Linux、Windows（Slint + winit） |

Viewer 会在子进程中设置 `DFT_UNSTABLE=yes`，因为 JSON 输出在上游仍为 **实验性功能**。

## 平台支持

本 crate **跨平台**（macOS、Linux、Windows），viewer 本身没有仅 Unix 可用的 GUI 或 I/O 路径。

| 方面 | Windows 处理（源码） |
|------|----------------------|
| GUI | Slint `backend-winit` + `renderer-femtovg`（支持 Windows） |
| Release 二进制 | `windows_subsystem = "windows"` — 启动时不额外弹出控制台 |
| 查找 `difft` | `difft.exe` 名称、`where difft`、`DIFT_PATH` 可带 `.exe` 后缀 |
| 子进程 | `CREATE_NO_WINDOW`，避免弹出控制台（`difft_probe.rs`） |
| 路径 / CLI | `std::path` + `args_os()`，无硬编码 `/` 分隔符 |
| 安装提示 | `install_message()` 含 Windows 说明（`winget`、`scoop`、`choco`） |

工作区 CI 矩阵包含 `x86_64-pc-windows-msvc` 与 `aarch64-pc-windows-msvc`，作为 workspace 成员会在 Windows 上参与 `cargo test` 构建。

## 构建

在仓库根目录：

```bash
cargo build -p difftastic -p difft-file-viewer
```

产物：

- `target/debug/difft`（Windows 为 `.exe`）
- `target/debug/difft-file-viewer`（Windows 为 `.exe`）

Windows（PowerShell 或 cmd）命令相同：

```powershell
cargo build -p difftastic -p difft-file-viewer
set DIFT_PATH=%CD%\target\debug\difft.exe
target\debug\difft-file-viewer.exe file-a file-b
```

Release：

```bash
cargo build --release -p difftastic -p difft-file-viewer
```

## 安装 `difft`

查找顺序：

1. 环境变量 `DIFT_PATH`
2. 与 viewer 同目录下的 `difft` / `difft.exe`
3. `PATH` 中的 `difft`（`which` / `where`）

未找到时，界面会显示安装提示。

**macOS**

```bash
brew install difftastic
# 或在本仓库构建：
cargo build -p difftastic
export DIFT_PATH="$(pwd)/target/debug/difft"
```

**Windows**

```powershell
winget install Wilfred.difftastic
# 或：
cargo build -p difftastic
set DIFT_PATH=%CD%\target\debug\difft.exe
```

**Linux** — 使用发行版包管理器，或源码构建后设置 `DIFT_PATH`。

验证：

```bash
difft --version
```

## 使用

### 命令行

必须提供两个文件路径；可选第三个路径启用 **A | C | B** 三栏模式（`file-c` 不存在时会创建）：

```bash
difft-file-viewer <file-a> <file-b> [<file-c>]
```

示例：

```bash
cargo run -p difft-file-viewer -- sample_files/context_1.rs sample_files/context_2.rs
cargo run -p difft-file-viewer -- sample_files/context_1.rs sample_files/context_2.rs /path/to/merged.rs
```

传入路径后，启动时自动开始 diff。

`cargo run` 时路径前需加 `--`：

```bash
cargo run -p difft-file-viewer -- path/to/old.rs path/to/new.rs
```

### 双栏（两个路径）

```bash
difft-file-viewer file-a file-b
```

- 工具栏：**A | B**
- 仅对比 `file-a` 与 `file-b`
- 可点击 A/B **gutter** 行号高亮语法块（无 Apply）

### 三栏（三个路径）

```bash
difft-file-viewer file-a file-b file-c
```

- 工具栏：**A | C | B**（CLI 顺序：第 1、3、2 个参数）
- Diff 仍为 **A vs B**；**C** 列按行号显示 `file-c`
- `file-c` 不存在时会**创建**（空文件；必要时创建父目录）
- 在 A/B 选中语法块后，点击 **Apply**，再点击 **file C** 行号插入（**Esc** 取消）。

## C/C++ 格式化（`clang-format`）

调用 `difft` 前，可能为 **A / B** 生成格式化副本（仅用于 diff，不改原文件）。

### 触发条件

须**同时**满足：

| 条件 | 说明 |
|------|------|
| 文件类型 | `file-a` / `file-b` 为 C/C++（如 `.c`、`.cpp`、`.h`、`.hpp` 等） |
| 配置 | **当前工作目录**下存在非空 **`./.clangformat`** |
| 工具 | `PATH` 中有 `clang-format`（`clang-format --version` 成功） |
| 文件存在 | 路径在磁盘上已存在 |

任一不满足则**静默跳过**格式化，照常对原文件 diff。`file-c` 不参与格式化（diff 仍为 A vs B）。

### 配置文件名

使用 **`.clangformat`**（无连字符），**不是**常见的 **`.clang-format`**，以免误用项目里已有的 `.clang-format`。

本仓库提供 **[`.clangformat.example`](.clangformat.example)**（基于 Google、100 列）。请复制到启动 viewer 时的工作目录：

```bash
cp .clangformat.example .clangformat
```

按需编辑 `.clangformat` 以匹配团队风格；示例文件不会被自动读取。

### 实际执行的命令

对每个需要格式化的文件：

```text
clang-format -style=file:<cwd>/.clangformat <file>  →  ./.clangformat.cache.d/<hash>.<ext>
```

格式化结果写入 cwd 下的 **`.clangformat.cache.d/`**，`difft` 读副本；**`file-a` / `file-b` 原文件不变**。

### 缓存（跳过已处理文件）

为避免每次启动都跑 `clang-format`，在当前工作目录维护：

- **`./.clangformat.cache`**（JSON 索引）
- **`./.clangformat.cache.d/`**（格式化副本）

| 缓存字段 | 含义 |
|----------|------|
| `config` | 上次处理时 `.clangformat` 的 mtime |
| `entries[path]` | 该**源文件**上次成功生成副本时的 mtime |

启动时对每个 C/C++ 的 A/B 输入：

- **配置与源文件 mtime 均与缓存一致**，且副本文件存在 → 跳过 `clang-format`
- **`.clangformat` mtime 变化** → 清空索引并删除 `.clangformat.cache.d/`，全部重新生成
- **某源文件 mtime 变化** → 仅该文件重新生成副本，成功后更新缓存

**强制全量重跑：** 删除 `.clangformat.cache` 与 `.clangformat.cache.d/`，或 touch 配置/源文件使 mtime 变化。

### 注意事项与限制

- 请在放有 `.clangformat` 的目录下启动 viewer（先 `cd`）；配置**不是**按每个源文件路径向上查找。
- **不修改** `file-a` / `file-b` 原文件；仅 diff 使用缓存副本。Apply 写入 `file-c` 的内容来自 diff 视图（若启用格式化，即为格式化后的文本）。
- 缓存仅比较 **mtime**（不算内容 hash）；极短时间内多次改写且 mtime 未变时可能误判，可删缓存目录。
- 格式化失败会在紫色信息区提示；diff 仍基于**原文件**继续。

## 快捷键

焦点需在 diff 代码区（diff 完成后会自动聚焦）。快捷键沿用常见 Vim 风格。

在 macOS 上，下表中的 **Ctrl** 对相同操作也匹配 **⌘ (Meta)**。

### 滚动

| 按键 | 作用 |
|------|------|
| `Page Up` | 向上翻一整页 |
| `Page Down` | 向下翻一整页 |
| `Ctrl+b` | 向上翻一整页 |
| `Ctrl+f` | 向下翻一整页 |
| `Ctrl+u` | 向上翻半页 |
| `Ctrl+d` | 向下翻半页 |
| `Home` | 滚到文件顶部 |
| `End` | 滚到文件底部 |
| `G` 或 `Shift+g` | 滚到文件底部 |
| `g` 再 `g` | 滚到文件顶部（连按两次 `g`） |
| `h` | 代码区**向左**滚（长行；行号 gutter 固定） |
| `l` | 代码区**向右**滚 |

行号与 Apply 在固定 **gutter** 列；仅代码 pane 水平滚动。

### 字号

| 按键 | 作用 |
|------|------|
| `Ctrl+=` 或 `Ctrl++` / **⌘=** / **⌘+** | 增大代码字号（8–24 px） |
| `Ctrl+-` / **⌘-** | 减小代码字号 |

行高、gutter 宽度与水平滚动步长会随字号同步缩放。

### 选中与 Apply（仅三栏）

**仅**在传入三个路径（`file-a`、`file-b`、`file-c`）时可用。两个参数时无 Apply / Apply 撤销。

| 按键 / 操作 | 作用 |
|-------------|------|
| 点击 A 或 B 侧行号（gutter） | 选中该行所在语法块；再次点击同一行号取消选中 |
| `Escape` | 取消语法块选中 |
| **Apply**（选中块首行 gutter 内的黄色按钮） | 进入插入模式：点击 **file C** 行号插入块（原行后移），**Esc** 取消 |
| `u` 或 **Ctrl+Z** / **⌘Z** | 撤销最近一次已完成的 **Apply**（同时取消待插入状态；最多 100 步） |

不带 Ctrl/Meta 的 `u` 仅撤销 **Apply**；**Ctrl+Z** / **⌘Z** 相同。**Ctrl+u** / **⌘u** 仍为向上半页滚动。

### 退出

| 按键 | 作用 |
|------|------|
| `q` | 退出 viewer |

三栏模式下，若 **Apply** 已修改 `file-c`，退出前会先写回磁盘；路径为 C/C++ 且 cwd 已配置 `.clangformat` 时，还会在独立子进程中执行 `clang-format -i`。

## JSON 格式（重要）

依赖 `difft --display json`。上游标注为 **实验性**：

- 需要 `DFT_UNSTABLE=yes`（viewer 已自动设置）。
- **无稳定性保证** — 字段名与结构可能在版本间变化。
- JSON 中 **没有 schema 版本号**。

单文件 diff 时，stdout 为一个 **object**，主要字段：

- `path`、`language`、`status`（`changed` / `created` / `deleted` / `unchanged`）
- `extra_info`（可选）
- `aligned_lines[]`：`lhs_text`、`rhs_text`、`is_novel_lhs`、`is_novel_rhs` 及可选 span 信息

viewer 会忽略未知字段；缺少或重命名必需字段会导致解析失败。

**建议：** 部署时让 `difft` 与 viewer **同一仓库 revision 构建**；升级 `difft` 后重新冒烟测试。

## 行为说明

| 项目 | 说明 |
|------|------|
| 范围 | 命令行 2 或 3 个**文件**（非目录） |
| Diff 对象 | 始终 `file-a` vs `file-b`；`file-c` 仅 viewer 使用 |
| 文件大小 | `--byte-limit 32000000`（32 MiB） |
| 上下文 | `--context 999999`（GUI 中 essentially 全文件） |
| 行号 | 界面 1-based；JSON 行号为 0-based |
| 长行 | `h` / `l` 水平滚动；gutter 不随动 |
| 警告 | Text fallback、byte limit、`clang-format` 错误等 — 紫色信息区 |

## 故障排查

| 现象 | 可能原因 |
|------|----------|
| `difft not found` | 未安装 `difft` 或未设置 `DIFT_PATH` |
| 紫色 JSON 解析错误 | `difft` 版本过旧或 JSON 格式变化 — 同仓库重建 |
| 内容区空白 | 查看紫色错误；检查路径与编码 |
| 未执行 `clang-format` | cwd 无 `.clangformat`、配置为空、非 C/C++、或找不到 `clang-format` |
| 格式化被错误跳过 | 删除 `.clangformat.cache` 或改变文件/配置 mtime |
| Windows 闪控制台 | 子进程已设 `CREATE_NO_WINDOW`；仍出现请反馈 |

## 许可证

MIT — 与 difftastic 相同。
