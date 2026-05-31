# difft-file-viewer

[English](README.md) | [简体中文](README.zh-CN.md)

Slint GUI for comparing two files, or applying two files to a third, with [difftastic](https://github.com/wilfred/difftastic).

The viewer optionally runs `clang-format` on C/C++ inputs, then spawns `difft`, reads `--display json` output, and renders a side-by-side (or triple-pane) view.

## How it works

```
                    ┌─ optional: clang-format copies ─┐
                    │  (.clangformat in cwd)          │
                    ▼                             │
┌─────────────┐     subprocess      ┌──────────┐  │
│ difft-file- │  DFT_UNSTABLE=yes   │  difft   │  │
│ viewer      │ ──────────────────► │  (CLI)   │  │
└─────────────┘  --display json     └──────────┘  │
        │                                      │    │
        │         stdout: JSON (one object)    │    │
        └──────────────────────────────────────┘    │
                                                    │
  C/C++ input paths ◄───────────────────────────────┘
```

1. **Pre-process (optional):** for each existing C/C++ **diff input** (`file-a` / `file-b`), if the **current working directory** contains a non-empty **`.clangformat`** and `clang-format` is on `PATH`, build formatted **copies** for `difft` without modifying the originals on disk. See [C/C++ formatting](#cc-formatting-clang-format) below.
2. **Diff:** the viewer runs (paths are `file-a` and `file-b` only; `file-c` is not passed to `difft`):

   ```text
   difft --display json --byte-limit 32000000 --context 999999 <file-a> <file-b>
   ```

3. JSON is parsed into aligned lines (`lhs_text`, `rhs_text`, novelty flags, syntax spans/blocks).
4. Slint shows:
   - **Two arguments:** **A | B** dual pane (red = removed/changed on A, green = added/changed on B).
   - **Three arguments:** **A | C | B** — same diff for A vs B; **C** shows `file-c` by line index (opened or created if missing). Syntax-block **Apply** writes selected hunks into C.

On success, status messages are hidden. Errors, `clang-format` warnings, and diff fallback messages appear in the purple info area.

## Requirements

| Component | Version / notes |
|-----------|-----------------|
| Rust | 1.85+ (see `rust-version` in `Cargo.toml`) |
| `difft` | Same release as this crate (currently **0.70.0**) recommended |
| `clang-format` | Optional — only for the [C/C++ pre-process](#cc-formatting-clang-format) |
| OS | macOS, Linux, Windows (Slint + winit) |

The viewer sets `DFT_UNSTABLE=yes` on the subprocess because JSON output is still an **unstable** difftastic feature.

## Platform support

The crate is **cross-platform** (macOS, Linux, Windows). There is no Unix-only GUI or I/O path in the viewer itself.

| Area | Windows handling (in source) |
|------|------------------------------|
| GUI | Slint `backend-winit` + `renderer-femtovg` (Windows supported) |
| Release binary | `windows_subsystem = "windows"` — no extra console window on launch |
| `difft` lookup | `difft.exe` name, `where difft`, optional `.exe` suffix on `DIFT_PATH` |
| Subprocess | `CREATE_NO_WINDOW` so `difft` does not flash a console (`difft_probe.rs`) |
| Paths / CLI | `std::path` + `args_os()` — no hard-coded `/` separators |
| Install hints | Windows-specific message (`winget`, `scoop`, `choco`) in `install_message()` |

The workspace CI matrix includes `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`, so `cargo test` builds this crate on Windows as a workspace member.

## Building

From the repository root:

```bash
cargo build -p difftastic -p difft-file-viewer
```

Binaries:

- `target/debug/difft` (`.exe` on Windows)
- `target/debug/difft-file-viewer` (`.exe` on Windows)

Windows (PowerShell or cmd), same commands:

```powershell
cargo build -p difftastic -p difft-file-viewer
set DIFT_PATH=%CD%\target\debug\difft.exe
target\debug\difft-file-viewer.exe file-a file-b
```

Release:

```bash
cargo build --release -p difftastic -p difft-file-viewer
```

## Installing `difft`

The viewer looks for `difft` in this order:

1. `DIFT_PATH` environment variable
2. `difft` / `difft.exe` next to the viewer executable
3. `difft` on `PATH` (`which` / `where`)

If `difft` is missing, the UI shows install hints.

**macOS**

```bash
brew install difftastic
# or build from this repo:
cargo build -p difftastic
export DIFT_PATH="$(pwd)/target/debug/difft"
```

**Windows**

```powershell
winget install Wilfred.difftastic
# or:
cargo build -p difftastic
set DIFT_PATH=%CD%\target\debug\difft.exe
```

**Linux** — use your package manager, or build from source and set `DIFT_PATH`.

Verify:

```bash
difft --version
```

## Usage

### Command line

Two paths are required; an optional third path enables the **A | C | B** triple-pane mode (`file-c` is opened or created):

```bash
difft-file-viewer <file-a> <file-b> [<file-c>]
```

Example:

```bash
cargo run -p difft-file-viewer -- sample_files/context_1.rs sample_files/context_2.rs
cargo run -p difft-file-viewer -- sample_files/context_1.rs sample_files/context_2.rs /path/to/merged.rs
```

With paths on the command line, diff starts automatically after launch.

Use `--` before paths when using `cargo run`:

```bash
cargo run -p difft-file-viewer -- path/to/old.rs path/to/new.rs
```

### Dual pane (two paths)

```bash
difft-file-viewer file-a file-b
```

- Toolbar: **A | B**
- Diff compares `file-a` vs `file-b` only.
- Click a **gutter** line number on A or B to highlight a syntax block (no Apply).

### Triple pane (three paths)

```bash
difft-file-viewer file-a file-b file-c
```

- Toolbar: **A | C | B** (CLI order: 1st, 3rd, 2nd path).
- Diff is still **A vs B**; column **C** shows `file-c` aligned by line number.
- If `file-c` does not exist, it is **created** (empty file; parent directories are created if needed).
- Select a syntax block on A or B, click **Apply**, then click a **file C** line number to insert the block (or **Esc** to cancel).

## C/C++ formatting (`clang-format`)

Before calling `difft`, the viewer may build formatted copies for **A / B** (diff only; originals are not modified).

### When it runs

All of the following must be true:

| Condition | Detail |
|-----------|--------|
| File type | `file-a` / `file-b` are C/C++ (e.g. `.c`, `.cpp`, `.h`, `.hpp`, …) |
| Config | **`./.clangformat`** exists in the **current working directory**, is non-empty |
| Tool | `clang-format` is available on `PATH` (`clang-format --version` succeeds) |
| File exists | The path already exists on disk |

If any condition fails, formatting is **silently skipped** and diff uses the original files. `file-c` is never formatted (diff is still A vs B).

### Config file name

The viewer reads **`.clangformat`** (no hyphen), **not** the usual **`.clang-format`**. This avoids clashing with an existing project `.clang-format` you do not want this tool to use.

Example `./.clangformat`:

```yaml
BasedOnStyle: LLVM
IndentWidth: 4
```

### Command executed

For each file that needs formatting:

```text
clang-format -style=file:<cwd>/.clangformat <file>  →  ./.clangformat.cache.d/<hash>.<ext>
```

Output goes under **`.clangformat.cache.d/`** in the cwd; `difft` reads the copies. **`file-a` / `file-b` on disk are unchanged.**

### Cache (skip already formatted files)

To avoid running `clang-format` on every launch, the viewer maintains:

- **`./.clangformat.cache`** (JSON index)
- **`./.clangformat.cache.d/`** (formatted copies)

| Cached field | Meaning |
|--------------|---------|
| `config` | mtime of `.clangformat` when last processed |
| `entries[path]` | mtime of the **source file** when its copy was last generated |

On startup, for each C/C++ A/B input:

- If **both** config and source mtimes match the cache **and** the copy file exists → **skip** `clang-format`.
- If `.clangformat` mtime changed → index is cleared and `.clangformat.cache.d/` is removed; all copies are regenerated.
- If a source file’s mtime changed → that file’s copy is regenerated; cache is updated after success.

**Force re-format:** delete `.clangformat.cache` and `.clangformat.cache.d/` (or touch `.clangformat` / the source file so mtime differs).

### Notes and caveats

- Run the viewer from the directory where `.clangformat` lives (`cd` there first). The config path is **not** discovered relative to each source file.
- **Does not modify** `file-a` / `file-b` on disk; only diff uses cached copies. Apply writes text from the diff view into `file-c` (formatted text when formatting is enabled).
- Cache uses **mtime only** (not content hash). Two edits within the same filesystem timestamp granularity could theoretically be missed; delete the cache directory if you suspect stale skips.
- Format failures are shown in the purple info area; diff still runs on the **original** files.

## Keyboard shortcuts

Focus must be on the diff panel (it receives focus after a diff finishes). Shortcuts follow common Vim-style bindings.

On macOS, **Ctrl** in the table below also matches **⌘ (Meta)** for the same actions.

### Scrolling

| Key | Action |
|-----|--------|
| `Page Up` | Scroll up one page |
| `Page Down` | Scroll down one page |
| `Ctrl+b` | Scroll up one page |
| `Ctrl+f` | Scroll down one page |
| `Ctrl+u` | Scroll up half a page |
| `Ctrl+d` | Scroll down half a page |
| `Home` | Scroll to top |
| `End` | Scroll to bottom |
| `G` or `Shift+g` | Scroll to bottom |
| `g` then `g` | Scroll to top (press `g` twice) |
| `h` | Scroll code **left** (long lines; gutter stays fixed) |
| `l` | Scroll code **right** |

Line numbers and Apply sit in a fixed **gutter** column; only the code pane scrolls horizontally.

### Selection and Apply (triple-pane only)

These apply **only** when three file paths were given (`file-a`, `file-b`, `file-c`). With two paths, Apply and Apply-undo are disabled.

| Key / action | Action |
|--------------|--------|
| Click a line number (gutter) on A or B | Select the syntax block containing that line; click again to clear |
| `Escape` | Clear syntax-block selection |
| **Apply** (yellow button on the first line of a selected block) | Enter insert mode: click a **file C** line number to insert the block (existing lines shift down), or **Esc** to cancel |
| `u` or **Ctrl+Z** / **⌘Z** | Undo the most recent completed **Apply** (also cancels a pending insert); up to 100 steps |

Plain `u` (no Ctrl/Meta) undoes **Apply** only. **Ctrl+Z** / **⌘Z** does the same. **Ctrl+u** / **⌘u** still scrolls half a page up.

## JSON format (important)

The viewer depends on `difft --display json`. Upstream marks this as **experimental**:

- Requires `DFT_UNSTABLE=yes` (the viewer sets this automatically).
- **No stability guarantee** — field names and structure may change between difftastic releases.
- There is **no schema version** in the JSON.

Current shape (single file): one JSON **object** with fields such as:

- `path`, `language`, `status` (`changed` / `created` / `deleted` / `unchanged`)
- `extra_info` (optional)
- `aligned_lines[]` with `lhs_text`, `rhs_text`, `is_novel_lhs`, `is_novel_rhs`, plus optional span metadata

Extra JSON fields are ignored by the viewer. Missing or renamed fields can break parsing.

**Recommendation:** build `difft` and the viewer from the **same repository revision**, and re-test after upgrading `difft`.

## Behaviour notes

| Topic | Detail |
|-------|--------|
| Scope | Two or three **files** on the command line (not directories) |
| Diff pair | Always `file-a` vs `file-b`; `file-c` is viewer-only |
| File size | `--byte-limit 32000000` (32 MiB) |
| Context | `--context 999999` (essentially full file in the GUI) |
| Line numbers | Display is 1-based; JSON line indices are 0-based |
| Long lines | Horizontal scroll via `h` / `l`; gutter does not scroll |
| Warnings | Text fallback, byte limit, `clang-format` errors — purple info area |

## Troubleshooting

| Symptom | Likely cause |
|---------|----------------|
| `difft not found` | Install `difft` or set `DIFT_PATH` |
| Purple parse / JSON error | `difft` too old or JSON format changed — rebuild both from same repo |
| Empty panes | See purple error message; check paths and file encodings |
| `clang-format` not applied | No `.clangformat` in cwd, empty config, not C/C++, or `clang-format` not on `PATH` |
| Stale format skip | Delete `.clangformat.cache` or change file/config mtime |
| Windows console flash | Subprocess uses `CREATE_NO_WINDOW`; report if a console still appears |

## License

MIT — same as difftastic.
