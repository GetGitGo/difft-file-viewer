# difft-file-viewer

[English](README.md) | [简体中文](README.zh-CN.md)

跨平台 **Slint** GUI：并排对比两个文件，或在查看 **A vs B** 结构 diff 的同时编辑第三个文件（**file C**）。

本 viewer 基于 **[difftastic](https://github.com/wilfred/difftastic)**（Wilfred Hughes 的结构化 diff 工具）。本仓库 **自带 patched 源码，位于 `difftastic/`**（版本 **0.70.0**），含 GUI 扩展：

- `difft --display json` 输出 **`lhs_syntax_blocks`** / **`rhs_syntax_blocks`**，供 A/B gutter 语法块选择。
- `difft --dump-syntax-blocks <path>` 在编辑后导出 **file C** 的语法块。

`git clone` 本仓库后按下方步骤构建即可 — **请勿**用 Homebrew/winget 等上游 `difft` 替代（除非只需无补丁的普通 diff）。

## 基本机制

```
                    ┌─ 可选：clang-format 副本 ─┐
                    │  (cwd 下 .clangformat)    │
                    ▼                          │
┌─────────────┐     subprocess      ┌──────────┐  │
│ difft-file- │  DFT_UNSTABLE=yes   │  difft   │  │
│ viewer      │ ──────────────────► │（本仓库   │  │
└─────────────┘  --display json     │ difftastic│  │
        │                             └──────────┘  │
        │  解析 JSON + 从磁盘读 A/B 源文件           │
        │                                           │
        │  仅三栏模式：                              │
        │  difft --dump-syntax-blocks file-c ◄──────┘
        ▼
   Slint UI（A | C | B）
```

1. **前处理（可选）：** 对已存在的 C/C++ **diff 输入**（`file-a` / `file-b`），若**当前工作目录**有非空 **`.clangformat`** 且 `PATH` 中有 `clang-format`，则生成格式化**副本**供 `difft` 使用，**不修改原文件**。详见 [C/C++ 格式化](#c-c-格式化-clang-format)。
2. **Diff：** 仅对 **`file-a`、`file-b`** 调用 patched `difft`（`file-c` 不参与 diff）：

   ```text
   difft --display json --byte-limit 32000000 --context 999999 <file-a> <file-b>
   ```

3. 解析 JSON 得到对齐行、高亮 span、语法块；行文本从磁盘源文件读取（JSON 主要提供对齐与变更元数据）。
4. **三栏模式：** 加载或编辑 `file-c` 后刷新语法块：

   ```text
   difft --dump-syntax-blocks <file-c>
   ```

5. **Slint 界面：**
   - **两个参数：** **A | B** — 红/绿 novelty 高亮；点击 gutter 行号选中语法块。
   - **三个参数：** **A | C | B** — diff 仍为 A vs B；**C** 按对齐行号显示 `file-c`（不存在则创建）。在 A/B 选中块可 **Apply** 到 C；在 **C** 选中块可 **Delete** / **Move**。

成功时状态区隐藏；错误、`clang-format` 警告、diff fallback 等显示在紫色信息区。

## 环境要求

| 组件 | 说明 |
|------|------|
| Rust | 1.85+（见 `Cargo.toml` 中 `rust-version`） |
| `difft` | **随仓库提供**（`difftastic/` 目录，0.70.0 + GUI 补丁）；从源码构建 |
| `clang-format` | 可选 — 仅 [C/C++ 前处理](#c-c-格式化-clang-format) |
| 系统 | macOS、Linux、Windows（Slint + winit） |

Viewer 子进程会设置 `DFT_UNSTABLE=yes`（JSON 输出在上游仍为实验性功能）。

## 平台支持

| 方面 | Windows 处理 |
|------|----------------|
| GUI | Slint `backend-winit` + `renderer-femtovg` |
| Release | `windows_subsystem = "windows"`，启动无额外控制台 |
| 查找 `difft` | `difft.exe`、`where difft`、`DIFT_PATH` / `--difft` 可带 `.exe` |
| 子进程 | `CREATE_NO_WINDOW`，避免闪控制台 |
| 安装提示 | `winget`、`scoop`、`choco`（`install_message()`） |

## 构建

`git clone` 后先构建 **difft**，再构建 viewer：

```bash
cargo build --manifest-path difftastic/Cargo.toml
cargo build
```

产物（debug）：

- `difftastic/target/debug/difft`（Windows 为 `.exe`）
- `target/debug/difft-file-viewer`（Windows 为 `.exe`）

Release：

```bash
cargo build --release --manifest-path difftastic/Cargo.toml
cargo build --release
```

Windows（PowerShell）：

```powershell
cargo build --manifest-path difftastic/Cargo.toml
cargo build
.\target\debug\difft-file-viewer.exe --difft .\difftastic\target\debug\difft.exe file-a file-b file-c
```

在仓库根目录运行时，viewer 会自动查找 `difftastic/target/debug/difft`；也可用 **`--difft`** 或 **`DIFT_PATH`** 显式指定。

## 安装 / 定位 `difft`

查找顺序：

1. 命令行 **`--difft PATH`**
2. 环境变量 **`DIFT_PATH`**
3. 当前工作目录下 **`difftastic/target/debug|release/difft`**
4. viewer 同目录下的 `difft` / `difft.exe`
5. **`PATH`** 中的 `difft`（`which` / `where`）

若未找到 `difft`，或使用的是**未打补丁的上游版本**，gutter 语法块与 C 列编辑将无法正常工作。找不到可用二进制时，界面会显示安装提示。

验证：

```bash
difft --version
difft --dump-syntax-blocks --help   # 仅 patched 构建
```

## 使用

### 命令行

```bash
difft-file-viewer [--difft PATH] <file-a> <file-b> [<file-c>]
```

示例：

```bash
cargo run -- --difft difftastic/target/debug/difft old.rs new.rs
cargo run -- --difft difftastic/target/debug/difft old.cpp new.cpp merged.cpp
```

传入路径后，启动时自动开始 diff。

### 双栏（两个路径）

```bash
difft-file-viewer file-a file-b
```

- 工具栏：**A | B**
- 对比 `file-a` 与 `file-b`
- 点击 A/B **gutter** 行号高亮最内层语法块；再次点击取消

### 三栏（三个路径）

```bash
difft-file-viewer file-a file-b file-c
```

- 工具栏：**A | C | B**（CLI 顺序：第 1、3、2 个参数）
- Diff 仍为 **A vs B**；**C** 列按对齐行号显示 `file-c`
- `file-c` 不存在时会**创建**（空文件；必要时创建父目录）

**从 A/B 复制到 C：**

1. 在 A 或 B 点击 gutter 选中语法块。
2. 点击块首行黄色 **Apply**。
3. 点击 **file C** gutter 行号插入（悬停绿色 ▶），或 **Esc** 取消。

**在 file C 上编辑：**

1. 在 **C** 列 gutter 点击选中 `file-c` 中的语法块。
2. **Delete**（块首行红色按钮）— 清空块内行内容（保留行索引以对齐视图）。
3. **Move**（黄色按钮，位于 Delete **下方隔一行**）— 与 Apply 相同进入插入模式；点击 C gutter 选新位置，**Esc** 取消。

对 `file-c` 的写入会立即落盘。**`u`**、**Ctrl+Z** / **⌘Z** 可撤销最近一次 Apply / Delete / Move（最多 100 步）。

## C/C++ 格式化（`clang-format`）

调用 `difft` 前，可能为 **A / B** 生成格式化副本（不改原文件）。

### 触发条件

| 条件 | 说明 |
|------|------|
| 文件类型 | `file-a` / `file-b` 为 C/C++ |
| 配置 | **cwd** 下非空 **`./.clangformat`** |
| 工具 | `PATH` 中有 `clang-format` |
| 文件存在 | 路径已在磁盘上 |

任一不满足则跳过。`file-c` 不作为 diff 输入参与格式化。

### 配置文件名

使用 **`.clangformat`**（无连字符），**不是** **`.clang-format`**，避免误用项目已有配置。

复制 **[`.clangformat.example`](.clangformat.example)** 到启动 viewer 时的工作目录：

```bash
cp .clangformat.example .clangformat
```

### 缓存

副本在 **`.clangformat.cache.d/`**，索引 **`.clangformat.cache`**。删除二者可强制重跑。行为与英文版相同：按 mtime 跳过、配置变更清空缓存。

退出时，若已修改 `file-c` 且为 C/C++ 且 cwd 有 `.clangformat`，可能在独立进程中对 `file-c` 执行 `clang-format -i`。

## 快捷键

焦点需在 diff 代码区（diff 完成后自动聚焦）。macOS 上表内 **Ctrl** 亦匹配 **⌘ (Meta)**。

### 滚动

| 按键 | 作用 |
|------|------|
| `Page Up` / `Page Down` | 翻页 |
| `Ctrl+b` / `Ctrl+f` | 翻页 |
| `Ctrl+u` / `Ctrl+d` | 半页 |
| `Home` / `End`、`G`、`g` `g` | 顶 / 底 |
| `h` / `l` | 代码水平滚动（gutter 固定） |

### 字号

| 按键 | 作用 |
|------|------|
| `Ctrl+=` / `Ctrl++` | 增大代码字号（8–24 px） |
| `Ctrl+-` | 减小代码字号 |

### 选中与编辑（三栏）

| 按键 / 操作 | 作用 |
|-------------|------|
| A/B/C gutter 点击 | 选中语法块；再点取消 |
| `Escape` | 取消选中 / 取消待 Apply 或 Move |
| **Apply**（A/B） | 将 A/B 块插入 C |
| **Delete** / **Move**（C） | 删除或移动 C 中块 |
| `u` 或 **Ctrl+Z** / **⌘Z** | 撤销最近一次 C 列修改 |

单独 `u` 仅撤销 C 列编辑；**Ctrl+u** 仍为向上半页。

### 退出

| 按键 | 作用 |
|------|------|
| `q` | 退出（若已改 `file-c` 则先写盘） |

## JSON 格式（重要）

依赖 **patched** 的 `difft --display json` 与 `difft --dump-syntax-blocks`。

- 需要 `DFT_UNSTABLE=yes`（viewer 已自动设置）。
- **无稳定性保证**，字段可能随版本变化。
- JSON **无 schema 版本号**。

### Diff 输出（`--display json`）

主要结构（单个 JSON **object**）：

| 字段 | 含义 |
|------|------|
| `path`、`language`、`status` | 元数据（`changed` / `created` / `deleted` / `unchanged`） |
| `extra_info` | 可选说明 |
| `aligned_lines` | `[[lhs_index, rhs_index], …]`，0-based 行号 |
| `chunks` | 按对齐键关联的逐行变更（span、高亮） |
| `lhs_syntax_blocks`、`rhs_syntax_blocks` | gutter 可选语法块（**仅 patched difft**） |

A/B 行文本从**磁盘**读取；C 列从内存中的 `file-c` 读取。若 JSON 仍含旧版 embedded `lhs_text` / `rhs_text`，viewer 亦兼容。

### 单文件语法块（`--dump-syntax-blocks`）

```json
{
  "path": "file.c",
  "language": "C++",
  "syntax_blocks": [
    {"id": 37, "parent_id": 31, "label": "(if", "start_line": 10, "end_line": 25}
  ]
}
```

**建议：** 始终用本仓库 `difftastic/` 构建 `difft`，并通过 `--difft` 或 `DIFT_PATH` 指定。

## 行为说明

| 项目 | 说明 |
|------|------|
| 范围 | 命令行 2 或 3 个**文件**（非目录） |
| Diff 对象 | 始终 `file-a` vs `file-b`；`file-c` 仅 viewer 使用 |
| 文件大小 | `--byte-limit 32000000`（32 MiB） |
| 上下文 | `--context 999999`（GUI 中 essentially 全文件） |
| 行号 | 界面 1-based；JSON 0-based |
| Tab | 显示层展开 Tab（Courier New） |
| 长行 | `h` / `l` 水平滚动 |
| 警告 | Text fallback、byte limit、`clang-format` 等 — 紫色信息区 |

## 故障排查

| 现象 | 可能原因 |
|------|----------|
| `difft not found` | 构建 `difftastic/` 或设置 `--difft` / `DIFT_PATH` |
| 无语法块 / gutter 选不中 | 使用了未打补丁的上游 `difft` — 请用 `./difftastic/` 重建 |
| 紫色 JSON 解析错误 | `difft` 过旧或格式变化 — 重建 patched `difft` |
| C 列无 Delete/Move | 需三栏模式 + patched `--dump-syntax-blocks` |
| 未执行 `clang-format` | cwd 无 `.clangformat`、非 C/C++、或缺少工具 |
| 格式化缓存过期 | 删除 `.clangformat.cache` 与 `.clangformat.cache.d/` |

## 许可证

MIT — 与 difftastic 相同。

## 致谢

本项目的实现离不开以下优秀工作：

- **[difftastic](https://github.com/wilfred/difftastic)** — **[Wilfred Hughes](https://github.com/wilfred)** 的结构化 diff 引擎与 CLI，是对齐与解析的核心。difft-file-viewer 在本地 difftastic 克隆上扩展了面向 GUI 的语法块 JSON 导出。
- **[Slint](https://slint.dev/)** — 本 viewer 使用的跨平台声明式 UI 框架。
- **[tree-sitter](https://tree-sitter.github.io/)**（经 difftastic）— 多语言语法感知解析。
- **LLVM [clang-format](https://clang.llvm.org/docs/ClangFormat.html)** — 可选的 C/C++ 格式化集成。

感谢 difftastic 与 Slint 社区提供的工具与文档。
