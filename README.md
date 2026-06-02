# difft-file-viewer

[English](README.md) | [简体中文](README.zh-CN.md)

A cross-platform **Slint** GUI for comparing two files side by side, or editing a third file (**file C**) while viewing a structural diff between **A** and **B**.

The viewer is built on **[difftastic](https://github.com/wilfred/difftastic)** — the structural diff tool by Wilfred Hughes. This repository **includes a patched copy under `difftastic/`** (version **0.70.0**) with GUI-specific extensions:

- `difft --display json` includes **`lhs_syntax_blocks`** / **`rhs_syntax_blocks`** for gutter block selection on A/B.
- `difft --dump-syntax-blocks <path>` exports syntax blocks for **file C** after edits.

Clone this repo and build both crates below — **do not** substitute an upstream `difft` from Homebrew/winget unless you only need an unpatched diff.

## How it works

```
                    ┌─ optional: clang-format copies ─┐
                    │  (.clangformat in cwd)          │
                    ▼                                 │
┌─────────────┐     subprocess        ┌──────────┐   │
│ difft-file- │  DFT_UNSTABLE=yes     │  difft   │   │
│ viewer      │ ───────────────────► │ (patched │   │
└─────────────┘  --display json      │ difftastic)│  │
        │                             └──────────┘   │
        │  parse JSON + read A/B sources on disk     │
        │                                            │
        │  triple-pane only:                         │
        │  difft --dump-syntax-blocks file-c ◄───────┘
        ▼
   Slint UI (A | C | B)
```

1. **Pre-process (optional):** for existing C/C++ **diff inputs** (`file-a` / `file-b`), if the **current working directory** contains a non-empty **`.clangformat`** and `clang-format` is on `PATH`, build formatted **copies** for `difft` without modifying originals. See [C/C++ formatting](#cc-formatting-clang-format).
2. **Diff:** run patched `difft` on **`file-a` and `file-b` only** (`file-c` is not passed to diff):

   ```text
   difft --display json --byte-limit 32000000 --context 999999 <file-a> <file-b>
   ```

3. Parse JSON into aligned rows, syntax-highlight spans, and syntax blocks. Line text is read from the source files on disk (JSON carries alignment + change metadata).
4. **Triple-pane:** after loading or editing `file-c`, refresh its syntax blocks:

   ```text
   difft --dump-syntax-blocks <file-c>
   ```

5. **Slint UI:**
   - **Two paths:** **A | B** — red/green novelty highlighting; click gutter line numbers to select syntax blocks.
   - **Three paths:** **A | C | B** — same A vs B diff; **C** shows `file-c` by aligned line index (created if missing). Select blocks on A/B to **Apply** into C, or select blocks on **C** to **Delete** / **Move**.

On success, status messages are hidden. Errors, `clang-format` warnings, and diff fallback messages appear in the purple info area.

## Requirements

| Component | Version / notes |
|-----------|-----------------|
| Rust | 1.85+ (see `rust-version` in `Cargo.toml`) |
| `difft` | **Included in `difftastic/`** in this repo (0.70.0 + GUI patches); build from source |
| `clang-format` | Optional — [C/C++ pre-process](#cc-formatting-clang-format) only |
| OS | macOS, Linux, Windows (Slint + winit) |

The viewer sets `DFT_UNSTABLE=yes` because JSON output is still an **unstable** difftastic feature.

## Platform support

| Area | Windows handling |
|------|------------------|
| GUI | Slint `backend-winit` + `renderer-femtovg` |
| Release binary | `windows_subsystem = "windows"` — no extra console on launch |
| `difft` lookup | `difft.exe`, `where difft`, optional `.exe` on `DIFT_PATH` / `--difft` |
| Subprocess | `CREATE_NO_WINDOW` so `difft` does not flash a console |
| Install hints | `winget`, `scoop`, `choco` in `install_message()` |

## Building

After `git clone`, build **difft** first, then the viewer:

```bash
cargo build --manifest-path difftastic/Cargo.toml
cargo build
```

Binaries (debug):

- `difftastic/target/debug/difft` (`.exe` on Windows)
- `target/debug/difft-file-viewer` (`.exe` on Windows)

Release:

```bash
cargo build --release --manifest-path difftastic/Cargo.toml
cargo build --release
```

Windows (PowerShell):

```powershell
cargo build --manifest-path difftastic/Cargo.toml
cargo build
.\target\debug\difft-file-viewer.exe --difft .\difftastic\target\debug\difft.exe file-a file-b file-c
```

The viewer auto-discovers `difftastic/target/debug/difft` when run from the repo root; you can also pass **`--difft`** or set **`DIFT_PATH`** explicitly.

## Installing / locating `difft`

The viewer resolves `difft` in this order:

1. `--difft PATH` on the command line
2. `DIFT_PATH` environment variable
3. `difftastic/target/debug|release/difft` relative to the current working directory
4. `difft` next to the viewer executable
5. `difft` on `PATH` (`which` / `where`)

If `difft` is missing or is an **unpatched** upstream build, gutter syntax blocks and file C editing will not work correctly. The UI shows install hints when no working binary is found.

Verify:

```bash
difft --version
difft --dump-syntax-blocks --help   # patched build only
```

## Usage

### Command line

```bash
difft-file-viewer [--difft PATH] <file-a> <file-b> [<file-c>]
```

Examples:

```bash
cargo run -- --difft difftastic/target/debug/difft old.rs new.rs
cargo run -- --difft difftastic/target/debug/difft old.cpp new.cpp merged.cpp
```

With paths on the command line, diff starts automatically after launch.

### Dual pane (two paths)

```bash
difft-file-viewer file-a file-b
```

- Toolbar: **A | B**
- Diff compares `file-a` vs `file-b`.
- Click a **gutter** line number on A or B to highlight the innermost syntax block; click again to clear.

### Triple pane (three paths)

```bash
difft-file-viewer file-a file-b file-c
```

- Toolbar: **A | C | B** (CLI order: 1st, 3rd, 2nd path).
- Diff is still **A vs B**; column **C** shows `file-c` at the aligned line index.
- If `file-c` does not exist, it is **created** (empty file; parent directories are created if needed).

**From A or B (copy into C):**

1. Click a gutter line on A or B to select a syntax block.
2. Click the yellow **Apply** button on the block’s first line.
3. Click a **file C** gutter line to insert (green ▶ hover), or **Esc** to cancel.

**On file C (edit in place):**

1. Click a gutter line on **C** to select a syntax block in `file-c`.
2. **Delete** (red button on the block’s first line) — clears the block’s lines (keeps row indices for alignment).
3. **Move** (yellow button, **two lines below Delete**) — enter insert mode like Apply; click a C gutter line for the new position, or **Esc** to cancel.

All writes to `file-c` are saved immediately. **`u`**, **Ctrl+Z** / **⌘Z** undo the last Apply / Delete / Move (up to 100 steps).

## C/C++ formatting (`clang-format`)

Before calling `difft`, the viewer may build formatted copies for **A / B** only (originals are not modified).

### When it runs

| Condition | Detail |
|-----------|--------|
| File type | `file-a` / `file-b` are C/C++ (`.c`, `.cpp`, `.h`, `.hpp`, …) |
| Config | **`./.clangformat`** exists in the **current working directory**, non-empty |
| Tool | `clang-format` on `PATH` |
| File exists | Path already on disk |

If any condition fails, formatting is skipped. `file-c` is not used for diff input formatting.

### Config file name

The viewer reads **`.clangformat`** (no hyphen), **not** **`.clang-format`**, to avoid clashing with an existing project config you do not want this tool to pick up.

Copy **[`.clangformat.example`](.clangformat.example)** into the directory from which you launch the viewer:

```bash
cp .clangformat.example .clangformat
```

### Cache

Formatted copies live under **`.clangformat.cache.d/`** with index **`.clangformat.cache`**. Delete both to force regeneration. See previous README behaviour: mtime-based skip, config change clears cache.

On quit, if `file-c` was modified and is C/C++ with `.clangformat` in cwd, a detached `clang-format -i` may run on `file-c`.

## Keyboard shortcuts

Focus must be on the diff panel (auto-focused after diff completes). On macOS, **Ctrl** below also matches **⌘ (Meta)** for the same actions.

### Scrolling

| Key | Action |
|-----|--------|
| `Page Up` / `Page Down` | Scroll one page |
| `Ctrl+b` / `Ctrl+f` | Scroll one page |
| `Ctrl+u` / `Ctrl+d` | Half page |
| `Home` / `End`, `G`, `g` `g` | Top / bottom |
| `h` / `l` | Scroll code horizontally (gutter fixed) |

### Font size

| Key | Action |
|-----|--------|
| `Ctrl+=` / `Ctrl++` | Increase code font (8–24 px) |
| `Ctrl+-` | Decrease code font |

### Selection and editing (triple-pane)

| Key / action | Action |
|--------------|--------|
| Gutter click on A/B/C | Select syntax block; click again to clear |
| `Escape` | Clear selection / cancel pending Apply or Move |
| **Apply** (A/B) | Insert selected A/B block into C |
| **Delete** / **Move** (C) | Delete block or relocate within C |
| `u` or **Ctrl+Z** / **⌘Z** | Undo last file C change |

Plain `u` undoes file C edits only; **Ctrl+u** still scrolls half a page up.

### Quit

| Key | Action |
|-----|--------|
| `q` | Quit (writes `file-c` if modified) |

## JSON format (important)

The viewer depends on **patched** `difft --display json` and `difft --dump-syntax-blocks`.

- Requires `DFT_UNSTABLE=yes` (set automatically).
- **No stability guarantee** — field names may change between releases.
- **No schema version** in the JSON.

### Diff output (`--display json`)

Primary shape (single JSON **object**):

| Field | Purpose |
|-------|---------|
| `path`, `language`, `status` | File metadata (`changed` / `created` / `deleted` / `unchanged`) |
| `extra_info` | Optional human-readable note |
| `aligned_lines` | `[[lhs_index, rhs_index], …]` — 0-based line indices |
| `chunks` | Per-line change metadata (spans, highlights) keyed by alignment |
| `lhs_syntax_blocks`, `rhs_syntax_blocks` | Gutter-selectable syntax tree spans (**patched difft only**) |

The viewer reads **line text from disk** for A/B (and from memory for C). Legacy JSON with embedded `lhs_text` / `rhs_text` per aligned row is still accepted if present.

### Single-file syntax blocks (`--dump-syntax-blocks`)

```json
{
  "path": "file.c",
  "language": "C++",
  "syntax_blocks": [
    {"id": 37, "parent_id": 31, "label": "(if", "start_line": 10, "end_line": 25}
  ]
}
```

**Recommendation:** always build `difft` from this repo’s `difftastic/` tree and pass it with `--difft` or `DIFT_PATH`.

## Behaviour notes

| Topic | Detail |
|-------|--------|
| Scope | Two or three **files** on the command line (not directories) |
| Diff pair | Always `file-a` vs `file-b`; `file-c` is viewer-only |
| File size | `--byte-limit 32000000` (32 MiB) |
| Context | `--context 999999` (essentially full file in the GUI) |
| Line numbers | Display 1-based; JSON indices 0-based |
| Tabs | Display expands tabs for alignment (Courier New) |
| Long lines | Horizontal scroll via `h` / `l` |
| Warnings | Text fallback, byte limit, `clang-format` errors — purple info area |

## Troubleshooting

| Symptom | Likely cause |
|---------|----------------|
| `difft not found` | Build `difftastic/` or set `--difft` / `DIFT_PATH` |
| No syntax blocks / empty gutter selection | Upstream `difft` without patches — rebuild from `./difftastic/` |
| Purple JSON parse error | `difft` too old or JSON changed — rebuild patched `difft` |
| `file-c` buttons missing | Need triple-pane mode and patched `--dump-syntax-blocks` |
| `clang-format` not applied | No `.clangformat` in cwd, not C/C++, or tool missing |
| Stale format cache | Delete `.clangformat.cache` and `.clangformat.cache.d/` |

## License

MIT — same as difftastic.

## Acknowledgments

This project would not exist without the excellent work of others:

- **[difftastic](https://github.com/wilfred/difftastic)** by **[Wilfred Hughes](https://github.com/wilfred)** — the structural diff engine and CLI that powers all parsing and alignment. difft-file-viewer extends a local difftastic clone with JSON syntax-block export for the GUI.
- **[Slint](https://slint.dev/)** — the declarative UI toolkit used for the cross-platform viewer.
- **[tree-sitter](https://tree-sitter.github.io/)** (via difftastic) — syntax-aware parsing for dozens of languages.
- **LLVM [clang-format](https://clang.llvm.org/docs/ClangFormat.html)** — optional C/C++ formatting integration.

Thank you to the difftastic and Slint communities for the tools and documentation that made this viewer possible.
