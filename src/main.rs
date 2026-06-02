#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Release Windows builds use the GUI subsystem (no extra console window).

mod clang_format_preprocess;
mod difft_probe;
mod line_ending;
mod model;
mod segments;
#[cfg(target_os = "macos")]
mod macos_icon;

slint::include_modules!();

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use difft_probe::{difft_command, install_message, resolve_difft};
use model::{
    gutter_syntax_block, parse_diff_json, parse_syntax_blocks_json, warning_message, AlignedLine,
    DiffFile, SyntaxBlock,
};
use segments::{
    build_segments, code_brush, plain_line_brush, prepare_display_line, text_pixel_width,
    to_slint_segments, Side, GUTTER_INSERT, GUTTER_LINE, GUTTER_SELECTED,
};

const BYTE_LIMIT: &str = "32000000";
/// Show essentially the whole file in the GUI (not just changed hunks).
const FULL_FILE_CONTEXT: &str = "999999";
const MAX_APPLY_HISTORY: usize = 100;

#[derive(Clone)]
struct ViewData {
    diff: DiffFile,
    file_c_lines: Vec<String>,
    file_c_syntax_blocks: Vec<SyntaxBlock>,
    triple_pane: bool,
}

struct CliArgs {
    path_a: PathBuf,
    path_b: PathBuf,
    path_c: Option<PathBuf>,
    difft: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffSide {
    Lhs,
    Rhs,
    Center,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BlockSelection {
    side: DiffSide,
    block_id: u32,
    start_line: u32,
    end_line: u32,
}

struct ApplyHistory {
    undo_stack: Vec<Vec<String>>,
}

impl ApplyHistory {
    fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.undo_stack.clear();
    }

    fn push_snapshot(&mut self, file_c_lines: &[String]) {
        self.undo_stack.push(file_c_lines.to_vec());
        if self.undo_stack.len() > MAX_APPLY_HISTORY {
            let overflow = self.undo_stack.len() - MAX_APPLY_HISTORY;
            self.undo_stack.drain(0..overflow);
        }
    }

    fn pop_snapshot(&mut self) -> Option<Vec<String>> {
        self.undo_stack.pop()
    }

    fn file_c_modified(&self) -> bool {
        !self.undo_stack.is_empty()
    }
}

fn cli_usage_error(got: usize) -> String {
    let detail = match got {
        0 => "at least two file paths are required.".to_string(),
        1 => "at least two file paths are required (got 1).".to_string(),
        n => format!("expected 2 or 3 file paths (got {n})."),
    };
    format!("{}\n\nError: {detail}", usage())
}

fn parse_cli_args() -> Result<CliArgs, String> {
    let mut difft = None;
    let mut paths = Vec::new();
    let mut args = env::args_os().skip(1);

    while let Some(arg) = args.next() {
        let key = arg.to_string_lossy();
        match key.as_ref() {
            "--help" | "-h" => return Err(usage()),
            "--difft" => {
                let Some(value) = args.next() else {
                    return Err(format!("--difft requires a path.\n\n{}", usage()));
                };
                difft = Some(PathBuf::from(value));
            }
            _ if key.starts_with("--difft=") => {
                let path = key.trim_start_matches("--difft=");
                if path.is_empty() {
                    return Err(format!("--difft requires a path.\n\n{}", usage()));
                }
                difft = Some(PathBuf::from(path));
            }
            _ if key.starts_with('-') => {
                return Err(format!("unknown option: {key}\n\n{}", usage()));
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }

    match paths.len() {
        2 => Ok(CliArgs {
            path_a: paths[0].clone(),
            path_b: paths[1].clone(),
            path_c: None,
            difft,
        }),
        3 => Ok(CliArgs {
            path_a: paths[0].clone(),
            path_b: paths[1].clone(),
            path_c: Some(paths[2].clone()),
            difft,
        }),
        got => Err(cli_usage_error(got)),
    }
}

fn usage() -> String {
    "Usage: difft-file-viewer [--difft PATH] <file-a> <file-b> [<file-c>]\n\
     \n\
     Options:\n\
       --difft PATH   Path to the difft binary (overrides DIFT_PATH and auto-discovery)\n\
       -h, --help     Show this help"
        .to_owned()
}

fn full_path(path: PathBuf) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    if path.is_absolute() {
        return path;
    }

    env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
}

fn open_or_create_file_lines(path: &Path) -> Result<Vec<String>, String> {
    if path.exists() {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        return Ok(if content.is_empty() {
            vec![]
        } else {
            content.lines().map(str::to_owned).collect()
        });
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("failed to create directory {}: {e}", parent.display())
            })?;
        }
    }
    fs::File::create(path).map_err(|e| format!("failed to create {}: {e}", path.display()))?;
    Ok(vec![])
}

fn write_file_lines(path: &Path, lines: &[String]) -> Result<(), String> {
    let content = if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    };
    fs::write(path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn block_source_lines(diff: &DiffFile, side: DiffSide, start: u32, end: u32) -> Vec<(u32, String)> {
    let mut lines = Vec::new();
    for aligned in &diff.aligned_lines {
        let line = match side {
            DiffSide::Lhs => aligned.lhs_line.map(|n| (n, aligned.lhs_text.clone())),
            DiffSide::Rhs => aligned.rhs_line.map(|n| (n, aligned.rhs_text.clone())),
            DiffSide::Center => None,
        };
        if let Some((line_no, text)) = line {
            if start <= line_no && line_no <= end {
                lines.push((line_no, text));
            }
        }
    }
    lines.sort_by_key(|(line_no, _)| *line_no);
    lines
}

fn is_empty_line(line: &str) -> bool {
    line.trim().is_empty()
}

/// Place one line at `pos`, overwriting empty slots and cascading displaced content
/// with the same empty-overwrite / non-empty-push strategy.
fn place_line_at(lines: &mut Vec<String>, pos: usize, text: String) {
    if pos >= lines.len() {
        lines.push(text);
        return;
    }
    if is_empty_line(&lines[pos]) {
        lines[pos] = text;
        return;
    }
    let displaced = std::mem::replace(&mut lines[pos], text);
    place_line_at(lines, pos + 1, displaced);
}

fn prepare_insert_point(file_c_lines: &mut Vec<String>, insert_at: usize) {
    if insert_at > file_c_lines.len() {
        file_c_lines.resize(insert_at, String::new());
    }
}

fn apply_block_to_file_c(
    file_c_lines: &mut Vec<String>,
    diff: &DiffFile,
    sel: BlockSelection,
    insert_at: usize,
) -> Result<(), String> {
    let block_lines: Vec<String> = block_source_lines(diff, sel.side, sel.start_line, sel.end_line)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    if block_lines.is_empty() {
        return Err("selected syntax block has no source lines.".to_owned());
    }

    prepare_insert_point(file_c_lines, insert_at);
    let mut pos = insert_at;
    for text in block_lines {
        place_line_at(file_c_lines, pos, text);
        pos += 1;
    }
    Ok(())
}

fn file_c_block_lines(file_c_lines: &[String], start: u32, end: u32) -> Vec<String> {
    let start = start as usize;
    if start >= file_c_lines.len() {
        return vec![];
    }
    let end = (end as usize).min(file_c_lines.len() - 1);
    if start > end {
        return vec![];
    }
    file_c_lines[start..=end].to_vec()
}

fn clear_block_in_file_c(file_c_lines: &mut [String], start: u32, end: u32) {
    let start = start as usize;
    if start >= file_c_lines.len() {
        return;
    }
    let end = (end as usize).min(file_c_lines.len() - 1);
    if start > end {
        return;
    }
    for line in &mut file_c_lines[start..=end] {
        line.clear();
    }
}

fn delete_block_from_file_c(
    file_c_lines: &mut Vec<String>,
    start: u32,
    end: u32,
) -> Result<(), String> {
    let block_lines = file_c_block_lines(file_c_lines, start, end);
    if block_lines.is_empty() {
        return Err("selected syntax block has no lines.".to_owned());
    }
    if block_lines.iter().all(|line| is_empty_line(line)) {
        return Err("selected syntax block is already empty.".to_owned());
    }
    clear_block_in_file_c(file_c_lines, start, end);
    Ok(())
}

fn move_block_in_file_c(
    file_c_lines: &mut Vec<String>,
    start: u32,
    end: u32,
    insert_at: usize,
) -> Result<(), String> {
    let block_lines = file_c_block_lines(file_c_lines, start, end);
    if block_lines.is_empty() {
        return Err("selected syntax block has no lines.".to_owned());
    }
    if block_lines.iter().all(|line| is_empty_line(line)) {
        return Err("selected syntax block is already empty.".to_owned());
    }
    clear_block_in_file_c(file_c_lines, start, end);
    prepare_insert_point(file_c_lines, insert_at);
    let mut pos = insert_at;
    for text in block_lines {
        place_line_at(file_c_lines, pos, text);
        pos += 1;
    }
    Ok(())
}

fn persist_file_c_edit(
    view: &mut ViewData,
    path_c: &Path,
    snapshot: &[String],
    apply_history: &Arc<Mutex<ApplyHistory>>,
    difft_store: &Arc<Mutex<Option<PathBuf>>>,
) -> Result<(), String> {
    write_file_lines(path_c, &view.file_c_lines)?;
    apply_history.lock().unwrap().push_snapshot(snapshot);
    if let Some(difft) = difft_store.lock().unwrap().clone() {
        refresh_file_c_syntax_blocks(view, &difft, path_c);
    }
    Ok(())
}

fn show_apply_on_line(
    triple_pane: bool,
    sel: Option<BlockSelection>,
    side: DiffSide,
    line: Option<u32>,
) -> bool {
    triple_pane
        && sel.is_some_and(|s| {
            s.side == side && line.is_some_and(|ln| ln == s.start_line)
        })
}

fn show_center_delete(
    triple_pane: bool,
    sel: Option<BlockSelection>,
    line: Option<u32>,
) -> bool {
    triple_pane
        && sel.is_some_and(|s| {
            s.side == DiffSide::Center && line.is_some_and(|ln| ln == s.start_line)
        })
}

fn show_center_move(
    triple_pane: bool,
    sel: Option<BlockSelection>,
    line: Option<u32>,
) -> bool {
    triple_pane
        && sel.is_some_and(|s| {
            s.side == DiffSide::Center
                && line.is_some_and(|ln| ln == s.start_line.saturating_add(2))
        })
}

fn center_line_for_row(view: &ViewData, row: usize) -> Option<u32> {
    let aligned = view.diff.aligned_lines.get(row)?;
    aligned.lhs_line.or(aligned.rhs_line)
}

fn center_row_matches_action(view: &ViewData, row: usize, sel: BlockSelection, move_action: bool) -> bool {
    let Some(line) = center_line_for_row(view, row) else {
        return false;
    };
    if move_action {
        line == sel.start_line.saturating_add(2)
    } else {
        line == sel.start_line
    }
}

fn run_difft(difft: &Path, path_a: &Path, path_b: &Path) -> Result<DiffFile, String> {
    let output = difft_command(difft)
        .env("DFT_UNSTABLE", "yes")
        .args([
            "--display",
            "json",
            "--byte-limit",
            BYTE_LIMIT,
            "--context",
            FULL_FILE_CONTEXT,
        ])
        .arg(path_a)
        .arg(path_b)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", difft.display()))?;

    if !output.stdout.is_empty() {
        return parse_diff_json(&output.stdout, path_a, path_b);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        return Err(stderr.trim().to_owned());
    }

    Err(format!(
        "difft exited with status {} and produced no output.",
        output.status
    ))
}

fn run_dump_syntax_blocks(difft: &Path, path: &Path) -> Result<Vec<SyntaxBlock>, String> {
    let output = difft_command(difft)
        .arg("--dump-syntax-blocks")
        .arg(path)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", difft.display()))?;

    if !output.stdout.is_empty() {
        return parse_syntax_blocks_json(&output.stdout).map(|file| file.syntax_blocks);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        return Err(stderr.trim().to_owned());
    }

    Err(format!(
        "difft --dump-syntax-blocks exited with status {} and produced no output.",
        output.status
    ))
}

fn refresh_file_c_syntax_blocks(view: &mut ViewData, difft: &Path, path_c: &Path) {
    view.file_c_syntax_blocks = run_dump_syntax_blocks(difft, path_c).unwrap_or_default();
}

/// Difft JSON line numbers are 0-based file indices; display 1-based like the terminal.
fn line_num(n: Option<u32>) -> i32 {
    n.map(|n| (n + 1) as i32).unwrap_or(-1)
}

fn line_in_selection(line: Option<u32>, sel: Option<BlockSelection>, side: DiffSide) -> bool {
    let Some(sel) = sel else {
        return false;
    };
    if sel.side != side {
        return false;
    }
    line.is_some_and(|ln| sel.start_line <= ln && ln <= sel.end_line)
}

fn center_line_for_aligned(aligned: &AlignedLine, file_c_lines: &[String]) -> (i32, String) {
    let Some(line_idx) = aligned.lhs_line.or(aligned.rhs_line) else {
        return (-1, String::new());
    };
    let text = file_c_lines
        .get(line_idx as usize)
        .cloned()
        .unwrap_or_default();
    (line_num(Some(line_idx)), text)
}

fn slint_line(
    line: &AlignedLine,
    view: &ViewData,
    sel: Option<BlockSelection>,
    apply_pending: bool,
) -> DiffLine {
    let (center_line, center_text_raw) = if view.triple_pane {
        center_line_for_aligned(line, &view.file_c_lines)
    } else {
        (-1, String::new())
    };
    let (lhs_text, lhs_spans) = prepare_display_line(&line.lhs_text, &line.lhs_spans);
    let (rhs_text, rhs_spans) = prepare_display_line(&line.rhs_text, &line.rhs_spans);
    let (center_text, center_spans) = prepare_display_line(&center_text_raw, &[]);

    DiffLine {
        lhs_novel: line.is_novel_lhs,
        rhs_novel: line.is_novel_rhs,
        lhs_line: line_num(line.lhs_line),
        rhs_line: line_num(line.rhs_line),
        center_line,
        lhs_selected: line_in_selection(line.lhs_line, sel, DiffSide::Lhs),
        rhs_selected: line_in_selection(line.rhs_line, sel, DiffSide::Rhs),
        center_selected: if apply_pending {
            center_line >= 1
        } else {
            line_in_selection(
                line.lhs_line.or(line.rhs_line),
                sel,
                DiffSide::Center,
            )
        },
        lhs_show_apply: show_apply_on_line(view.triple_pane, sel, DiffSide::Lhs, line.lhs_line),
        rhs_show_apply: show_apply_on_line(view.triple_pane, sel, DiffSide::Rhs, line.rhs_line),
        center_show_delete: show_center_delete(
            view.triple_pane,
            sel,
            line.lhs_line.or(line.rhs_line),
        ),
        center_show_move: show_center_move(
            view.triple_pane,
            sel,
            line.lhs_line.or(line.rhs_line),
        ),
        lhs_plain_text: lhs_text.clone().into(),
        rhs_plain_text: rhs_text.clone().into(),
        center_plain_text: center_text.clone().into(),
        lhs_plain_color: plain_line_brush(line.is_novel_lhs, Side::Left),
        rhs_plain_color: plain_line_brush(line.is_novel_rhs, Side::Right),
        center_plain_color: plain_line_brush(false, Side::Left),
        lhs_segments: to_slint_segments(&build_segments(
            &lhs_text,
            &lhs_spans,
            line.is_novel_lhs,
            Side::Left,
        )),
        rhs_segments: to_slint_segments(&build_segments(
            &rhs_text,
            &rhs_spans,
            line.is_novel_rhs,
            Side::Right,
        )),
        center_segments: to_slint_segments(&build_segments(
            &center_text,
            &center_spans,
            false,
            Side::Left,
        )),
        lhs_content_width: text_pixel_width(&lhs_text),
        rhs_content_width: text_pixel_width(&rhs_text),
        center_content_width: text_pixel_width(&center_text),
    }
}

fn max_line_content_width(lines: &[DiffLine]) -> f32 {
    lines.iter().fold(0.0f32, |max_width, line| {
        max_width
            .max(line.lhs_content_width)
            .max(line.rhs_content_width)
            .max(line.center_content_width)
    })
}

fn set_lines_on_ui(
    ui: &MainWindow,
    view: &ViewData,
    sel: Option<BlockSelection>,
    apply_pending: bool,
) {
    let lines = slint_lines(view, sel, apply_pending);
    ui.set_max_content_width(max_line_content_width(&lines));
    let model: slint::ModelRc<DiffLine> = std::rc::Rc::new(slint::VecModel::from(lines)).into();
    ui.set_lines(model);
}

fn init_gutter_colors(ui: &MainWindow) {
    ui.set_gutter_line_color(code_brush(GUTTER_LINE));
    ui.set_gutter_selected_color(code_brush(GUTTER_SELECTED));
    ui.set_gutter_insert_color(code_brush(GUTTER_INSERT));
}

fn refresh_diff_ui(
    ui: &MainWindow,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
) {
    let apply_pending = pending_apply_store.lock().unwrap().is_some();
    ui.set_apply_pending(apply_pending);
    if let Some(view) = view_store.lock().unwrap().clone() {
        let sel = *selection_store.lock().unwrap();
        set_lines_on_ui(ui, &view, sel, apply_pending);
    }
}

fn slint_lines(
    view: &ViewData,
    sel: Option<BlockSelection>,
    apply_pending: bool,
) -> Vec<DiffLine> {
    view.diff
        .aligned_lines
        .iter()
        .map(|line| slint_line(line, view, sel, apply_pending))
        .collect()
}

fn blocks_for_side<'a>(file: &'a DiffFile, side: DiffSide) -> &'a [SyntaxBlock] {
    match side {
        DiffSide::Lhs => &file.lhs_syntax_blocks,
        DiffSide::Rhs => &file.rhs_syntax_blocks,
        DiffSide::Center => &[],
    }
}

fn handle_gutter_click(
    ui: &MainWindow,
    row: i32,
    lhs_side: bool,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
) {
    if pending_apply_store.lock().unwrap().is_some() {
        return;
    }

    if row < 0 {
        *selection_store.lock().unwrap() = None;
        if let Some(view) = view_store.lock().unwrap().clone() {
            set_lines_on_ui(ui, &view, None, false);
        }
        return;
    }

    let Some(view) = view_store.lock().unwrap().clone() else {
        return;
    };

    let row = row as usize;
    let Some(aligned) = view.diff.aligned_lines.get(row) else {
        return;
    };

    let side = if lhs_side {
        DiffSide::Lhs
    } else {
        DiffSide::Rhs
    };
    let line_0based = match side {
        DiffSide::Lhs => aligned.lhs_line,
        DiffSide::Rhs => aligned.rhs_line,
        DiffSide::Center => return,
    };
    let Some(line_0based) = line_0based else {
        return;
    };

    let blocks = blocks_for_side(&view.diff, side);
    let Some(block) = gutter_syntax_block(blocks, line_0based) else {
        return;
    };

    let mut selection = selection_store.lock().unwrap();
    if selection.is_some_and(|prev| prev.side == side && prev.block_id == block.id) {
        *selection = None;
        set_lines_on_ui(ui, &view, None, false);
        return;
    }

    let new_sel = BlockSelection {
        side,
        block_id: block.id,
        start_line: block.start_line,
        end_line: block.end_line,
    };
    *selection = Some(new_sel);
    set_lines_on_ui(ui, &view, Some(new_sel), false);
}

fn handle_apply_click(
    ui: &MainWindow,
    row: i32,
    lhs_side: bool,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
) {
    if pending_apply_store.lock().unwrap().is_some() {
        return;
    }

    if row < 0 {
        return;
    }

    let Some(sel) = *selection_store.lock().unwrap() else {
        return;
    };

    let side = if lhs_side {
        DiffSide::Lhs
    } else {
        DiffSide::Rhs
    };
    if sel.side != side {
        return;
    }

    let Some(view) = view_store.lock().unwrap().clone() else {
        return;
    };
    if !view.triple_pane {
        return;
    }

    let row = row as usize;
    let Some(aligned) = view.diff.aligned_lines.get(row) else {
        return;
    };
    let block_start = match side {
        DiffSide::Lhs => aligned.lhs_line,
        DiffSide::Rhs => aligned.rhs_line,
        DiffSide::Center => return,
    };
    if block_start != Some(sel.start_line) {
        return;
    }

    *pending_apply_store.lock().unwrap() = Some(sel);
    refresh_diff_ui(ui, view_store, selection_store, pending_apply_store);
    ui.set_file_info(
        "Click a line number in file C to insert the selection, or press Esc to cancel.".into(),
    );
}

fn handle_center_apply_insert(
    ui: &MainWindow,
    row: i32,
    sel: BlockSelection,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
    path_c_store: &Arc<Mutex<Option<PathBuf>>>,
    apply_history: &Arc<Mutex<ApplyHistory>>,
    difft_store: &Arc<Mutex<Option<PathBuf>>>,
) {
    if row < 0 {
        *pending_apply_store.lock().unwrap() = Some(sel);
        return;
    }

    let Some(mut view) = view_store.lock().unwrap().clone() else {
        *pending_apply_store.lock().unwrap() = Some(sel);
        return;
    };
    if !view.triple_pane {
        *pending_apply_store.lock().unwrap() = Some(sel);
        return;
    }

    let row = row as usize;
    let Some(aligned) = view.diff.aligned_lines.get(row) else {
        *pending_apply_store.lock().unwrap() = Some(sel);
        return;
    };
    let insert_line = aligned.lhs_line.or(aligned.rhs_line);
    let Some(insert_line) = insert_line else {
        *pending_apply_store.lock().unwrap() = Some(sel);
        ui.set_file_info("Choose a file C row with a line number.".into());
        return;
    };
    let insert_at = insert_line as usize;

    let Some(path_c) = path_c_store.lock().unwrap().clone() else {
        *pending_apply_store.lock().unwrap() = Some(sel);
        ui.set_file_info("Apply requires a file-c path.".into());
        return;
    };

    let snapshot = view.file_c_lines.clone();
    match apply_block_to_file_c(&mut view.file_c_lines, &view.diff, sel, insert_at) {
        Ok(()) => {
            if let Err(err) = persist_file_c_edit(
                &mut view,
                &path_c,
                &snapshot,
                apply_history,
                difft_store,
            ) {
                view.file_c_lines = snapshot;
                *pending_apply_store.lock().unwrap() = Some(sel);
                ui.set_file_info(err.into());
                return;
            }
            *view_store.lock().unwrap() = Some(view.clone());
            let sel = *selection_store.lock().unwrap();
            ui.set_apply_pending(false);
            set_lines_on_ui(ui, &view, sel, false);
            ui.set_file_info(format!("Applied to {} at line {}.", path_c.display(), insert_at + 1).into());
        }
        Err(err) => {
            *pending_apply_store.lock().unwrap() = Some(sel);
            refresh_diff_ui(ui, view_store, selection_store, pending_apply_store);
            ui.set_file_info(err.into());
        }
    }
}

fn handle_center_block_select(
    ui: &MainWindow,
    row: i32,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
) {
    if row < 0 {
        *selection_store.lock().unwrap() = None;
        if let Some(view) = view_store.lock().unwrap().clone() {
            set_lines_on_ui(ui, &view, None, false);
        }
        return;
    }

    let Some(view) = view_store.lock().unwrap().clone() else {
        return;
    };
    if !view.triple_pane {
        return;
    }

    let row = row as usize;
    let Some(aligned) = view.diff.aligned_lines.get(row) else {
        return;
    };
    let Some(line_0based) = aligned.lhs_line.or(aligned.rhs_line) else {
        return;
    };

    let Some(block) = gutter_syntax_block(&view.file_c_syntax_blocks, line_0based) else {
        return;
    };

    let mut selection = selection_store.lock().unwrap();
    if selection.is_some_and(|prev| prev.side == DiffSide::Center && prev.block_id == block.id) {
        *selection = None;
        set_lines_on_ui(ui, &view, None, false);
        return;
    }

    let new_sel = BlockSelection {
        side: DiffSide::Center,
        block_id: block.id,
        start_line: block.start_line,
        end_line: block.end_line,
    };
    *selection = Some(new_sel);
    set_lines_on_ui(ui, &view, Some(new_sel), false);
}

fn handle_center_move_insert(
    ui: &MainWindow,
    row: i32,
    sel: BlockSelection,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
    path_c_store: &Arc<Mutex<Option<PathBuf>>>,
    apply_history: &Arc<Mutex<ApplyHistory>>,
    difft_store: &Arc<Mutex<Option<PathBuf>>>,
) {
    if row < 0 {
        *pending_apply_store.lock().unwrap() = Some(sel);
        return;
    }

    let Some(mut view) = view_store.lock().unwrap().clone() else {
        *pending_apply_store.lock().unwrap() = Some(sel);
        return;
    };
    if !view.triple_pane {
        *pending_apply_store.lock().unwrap() = Some(sel);
        return;
    }

    let row = row as usize;
    let Some(aligned) = view.diff.aligned_lines.get(row) else {
        *pending_apply_store.lock().unwrap() = Some(sel);
        return;
    };
    let insert_line = aligned.lhs_line.or(aligned.rhs_line);
    let Some(insert_line) = insert_line else {
        *pending_apply_store.lock().unwrap() = Some(sel);
        ui.set_file_info("Choose a file C row with a line number.".into());
        return;
    };
    let insert_at = insert_line as usize;

    let Some(path_c) = path_c_store.lock().unwrap().clone() else {
        *pending_apply_store.lock().unwrap() = Some(sel);
        ui.set_file_info("Move requires a file-c path.".into());
        return;
    };

    let snapshot = view.file_c_lines.clone();
    match move_block_in_file_c(
        &mut view.file_c_lines,
        sel.start_line,
        sel.end_line,
        insert_at,
    ) {
        Ok(()) => {
            if let Err(err) = persist_file_c_edit(
                &mut view,
                &path_c,
                &snapshot,
                apply_history,
                difft_store,
            ) {
                view.file_c_lines = snapshot;
                *pending_apply_store.lock().unwrap() = Some(sel);
                ui.set_file_info(err.into());
                return;
            }
            *view_store.lock().unwrap() = Some(view.clone());
            *selection_store.lock().unwrap() = None;
            ui.set_apply_pending(false);
            set_lines_on_ui(ui, &view, None, false);
            ui.set_file_info(
                format!(
                    "Moved block in {} to line {}.",
                    path_c.display(),
                    insert_at + 1
                )
                .into(),
            );
        }
        Err(err) => {
            *pending_apply_store.lock().unwrap() = Some(sel);
            refresh_diff_ui(ui, view_store, selection_store, pending_apply_store);
            ui.set_file_info(err.into());
        }
    }
}

fn handle_center_delete_click(
    ui: &MainWindow,
    row: i32,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    path_c_store: &Arc<Mutex<Option<PathBuf>>>,
    apply_history: &Arc<Mutex<ApplyHistory>>,
    difft_store: &Arc<Mutex<Option<PathBuf>>>,
) {
    if row < 0 {
        return;
    }

    let Some(sel) = *selection_store.lock().unwrap() else {
        return;
    };
    if sel.side != DiffSide::Center {
        return;
    }

    let Some(view) = view_store.lock().unwrap().clone() else {
        return;
    };
    if !view.triple_pane {
        return;
    }

    let row = row as usize;
    if !center_row_matches_action(&view, row, sel, false) {
        return;
    }

    let Some(path_c) = path_c_store.lock().unwrap().clone() else {
        ui.set_file_info("Delete requires a file-c path.".into());
        return;
    };

    let Some(mut view) = view_store.lock().unwrap().clone() else {
        return;
    };
    let snapshot = view.file_c_lines.clone();
    match delete_block_from_file_c(&mut view.file_c_lines, sel.start_line, sel.end_line) {
        Ok(()) => {
            if let Err(err) = persist_file_c_edit(
                &mut view,
                &path_c,
                &snapshot,
                apply_history,
                difft_store,
            ) {
                view.file_c_lines = snapshot;
                ui.set_file_info(err.into());
                return;
            }
            *view_store.lock().unwrap() = Some(view.clone());
            *selection_store.lock().unwrap() = None;
            set_lines_on_ui(ui, &view, None, false);
            ui.set_file_info(
                format!(
                    "Deleted block in {} (lines {}–{}).",
                    path_c.display(),
                    sel.start_line + 1,
                    sel.end_line + 1
                )
                .into(),
            );
        }
        Err(err) => ui.set_file_info(err.into()),
    }
}

fn handle_center_move_click(
    ui: &MainWindow,
    row: i32,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
) {
    if pending_apply_store.lock().unwrap().is_some() {
        return;
    }

    if row < 0 {
        return;
    }

    let Some(sel) = *selection_store.lock().unwrap() else {
        return;
    };
    if sel.side != DiffSide::Center {
        return;
    }

    let Some(view) = view_store.lock().unwrap().clone() else {
        return;
    };
    if !view.triple_pane {
        return;
    }

    let row = row as usize;
    if !center_row_matches_action(&view, row, sel, true) {
        return;
    }

    *pending_apply_store.lock().unwrap() = Some(sel);
    refresh_diff_ui(ui, view_store, selection_store, pending_apply_store);
    ui.set_file_info(
        "Click a line number in file C to move the block, or press Esc to cancel.".into(),
    );
}

fn handle_center_gutter_click(
    ui: &MainWindow,
    row: i32,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
    path_c_store: &Arc<Mutex<Option<PathBuf>>>,
    apply_history: &Arc<Mutex<ApplyHistory>>,
    difft_store: &Arc<Mutex<Option<PathBuf>>>,
) {
    if let Some(sel) = pending_apply_store.lock().unwrap().take() {
        match sel.side {
            DiffSide::Center => handle_center_move_insert(
                ui,
                row,
                sel,
                view_store,
                selection_store,
                pending_apply_store,
                path_c_store,
                apply_history,
                difft_store,
            ),
            _ => handle_center_apply_insert(
                ui,
                row,
                sel,
                view_store,
                selection_store,
                pending_apply_store,
                path_c_store,
                apply_history,
                difft_store,
            ),
        }
        return;
    }

    handle_center_block_select(ui, row, view_store, selection_store);
}

fn handle_apply_cancel(
    ui: &MainWindow,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
) {
    if pending_apply_store.lock().unwrap().take().is_none() {
        return;
    }
    refresh_diff_ui(ui, view_store, selection_store, pending_apply_store);
    ui.set_file_info("Cancelled.".into());
}

fn handle_quit_request(
    ui: &MainWindow,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    _selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
    path_c_store: &Arc<Mutex<Option<PathBuf>>>,
    apply_history: &Arc<Mutex<ApplyHistory>>,
) {
    pending_apply_store.lock().unwrap().take();
    ui.set_apply_pending(false);

    if apply_history.lock().unwrap().file_c_modified() {
        if let (Some(path_c), Some(view)) = (
            path_c_store.lock().unwrap().clone(),
            view_store.lock().unwrap().clone(),
        ) {
            if view.triple_pane {
                let _ = clang_format_preprocess::spawn_detached_format_file_c(
                    &path_c,
                    &view.file_c_lines,
                );
            }
        }
    }

    clang_format_preprocess::cleanup_cache();
    let _ = slint::quit_event_loop();
}

fn handle_apply_undo(
    ui: &MainWindow,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: &Arc<Mutex<Option<BlockSelection>>>,
    path_c_store: &Arc<Mutex<Option<PathBuf>>>,
    apply_history: &Arc<Mutex<ApplyHistory>>,
    difft_store: &Arc<Mutex<Option<PathBuf>>>,
) {
    pending_apply_store.lock().unwrap().take();
    ui.set_apply_pending(false);

    let Some(mut view) = view_store.lock().unwrap().clone() else {
        return;
    };
    if !view.triple_pane {
        return;
    }

    let Some(snapshot) = apply_history.lock().unwrap().pop_snapshot() else {
        ui.set_file_info("No Apply history to undo.".into());
        return;
    };

    let Some(path_c) = path_c_store.lock().unwrap().clone() else {
        ui.set_file_info("Undo requires a file-c path.".into());
        return;
    };

    view.file_c_lines = snapshot;
    if let Err(err) = write_file_lines(&path_c, &view.file_c_lines) {
        ui.set_file_info(err.into());
        return;
    }

    if let Some(difft) = difft_store.lock().unwrap().clone() {
        refresh_file_c_syntax_blocks(&mut view, &difft, &path_c);
    }

    *view_store.lock().unwrap() = Some(view.clone());
    let sel = *selection_store.lock().unwrap();
    set_lines_on_ui(ui, &view, sel, false);
    ui.set_file_info(format!("Undid last Apply on {}.", path_c.display()).into());
}

fn set_path_label(ui: &MainWindow, property: fn(&MainWindow, slint::SharedString), label: &str) {
    property(ui, label.into());
}

fn run_diff(
    ui_handle: slint::Weak<MainWindow>,
    difft: Arc<Mutex<Option<PathBuf>>>,
    path_a: PathBuf,
    path_b: PathBuf,
    path_c: Option<PathBuf>,
    triple_pane: bool,
    view_store: Arc<Mutex<Option<ViewData>>>,
    selection_store: Arc<Mutex<Option<BlockSelection>>>,
    pending_apply_store: Arc<Mutex<Option<BlockSelection>>>,
    apply_history: Arc<Mutex<ApplyHistory>>,
) {
    let difft_path = match difft.lock().unwrap().clone() {
        Some(path) => path,
        None => {
            if let Some(ui) = ui_handle.upgrade() {
                ui.set_diff_state(DiffState::Idle);
                ui.set_status_text("difft not found.".into());
                ui.set_file_info(install_message().into());
            }
            return;
        }
    };

    if let Some(ui) = ui_handle.upgrade() {
        ui.set_diff_state(DiffState::Diffing);
        ui.set_status_text("Diffing...".into());
        ui.set_file_info("".into());
    }

    std::thread::spawn(move || {
        let (diff_path_a, diff_path_b, preprocess_note) =
            clang_format_preprocess::diff_input_paths(&path_a, &path_b);

        let outcome: Result<ViewData, String> = (|| {
            let diff = run_difft(&difft_path, &diff_path_a, &diff_path_b)?;
            let path_c_ref = path_c.as_ref();
            let file_c_lines = if triple_pane {
                let path_c = path_c_ref.ok_or_else(|| "internal error: missing file-c path".to_string())?;
                open_or_create_file_lines(path_c)?
            } else {
                vec![]
            };
            let mut view = ViewData {
                diff,
                file_c_lines,
                file_c_syntax_blocks: vec![],
                triple_pane,
            };
            if let Some(path_c) = path_c_ref {
                refresh_file_c_syntax_blocks(&mut view, &difft_path, path_c);
            }
            Ok(view)
        })();

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_handle.upgrade() {
                match outcome {
                    Ok(view) => {
                        *selection_store.lock().unwrap() = None;
                        *pending_apply_store.lock().unwrap() = None;
                        apply_history.lock().unwrap().clear();
                        *view_store.lock().unwrap() = Some(view.clone());
                        ui.set_apply_pending(false);
                        set_lines_on_ui(&ui, &view, None, false);
                        ui.invoke_reset_diff_scroll();
                        ui.set_status_text("".into());
                        let mut info = warning_message(&view.diff).unwrap_or_default();
                        if let Some(note) = preprocess_note {
                            if !info.is_empty() {
                                info.push('\n');
                            }
                            info.push_str(&note);
                        }
                        ui.set_file_info(info.into());
                        ui.set_diff_state(DiffState::Diffed);
                        focus_diff_panel(&ui);
                    }
                    Err(err) => {
                        ui.set_diff_state(DiffState::Idle);
                        ui.set_status_text("".into());
                        ui.set_file_info(err.into());
                    }
                }
            }
        });
    });
}

fn focus_diff_panel(ui: &MainWindow) {
    ui.invoke_focus_diff_panel();
}

fn schedule_focus_diff_panel(ui: &MainWindow) {
    let ui_handle = ui.as_weak();
    let _ = slint::Timer::single_shot(Duration::from_millis(50), move || {
        if let Some(ui) = ui_handle.upgrade() {
            ui.invoke_focus_diff_panel();
        }
    });
}

fn maximize_on_startup(ui: &MainWindow) {
    let ui_handle = ui.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_handle.upgrade() {
            ui.window().set_maximized(true);
            schedule_focus_diff_panel(&ui);
        }
    });
}

#[cfg(target_os = "macos")]
fn schedule_application_icon() {
    let _ = slint::invoke_from_event_loop(move || {
        macos_icon::set_from_png(include_bytes!("../assets/icons/icon-512.png"));
    });
}

fn main() -> Result<(), slint::PlatformError> {
    let cli = parse_cli_args();
    let ui = MainWindow::new()?;
    init_gutter_colors(&ui);
    maximize_on_startup(&ui);
    #[cfg(target_os = "macos")]
    schedule_application_icon();

    let (path_a, path_b, path_c, label_a, label_b, label_c, triple_pane) = match &cli {
        Ok(args) => {
            ui.set_triple_pane(args.path_c.is_some());
            (
                Some(full_path(args.path_a.clone())),
                Some(full_path(args.path_b.clone())),
                args.path_c.clone().map(full_path),
                Some(args.path_a.display().to_string()),
                Some(args.path_b.display().to_string()),
                args.path_c
                    .as_ref()
                    .map(|p| p.display().to_string()),
                args.path_c.is_some(),
            )
        }
        Err(err) => {
            ui.set_triple_pane(false);
            ui.set_file_info(err.clone().into());
            (None, None, None, None, None, None, false)
        }
    };

    if let Some(label) = &label_a {
        set_path_label(&ui, MainWindow::set_path_a, label);
    }
    if let Some(label) = &label_b {
        set_path_label(&ui, MainWindow::set_path_b, label);
    }
    if let Some(label) = &label_c {
        set_path_label(&ui, MainWindow::set_path_c, label);
    }

    let difft = Arc::new(Mutex::new(
        cli.as_ref()
            .ok()
            .and_then(|args| args.difft.clone())
            .and_then(|path| resolve_difft(Some(path)).ok())
            .or_else(|| resolve_difft(None).ok()),
    ));
    let view_store: Arc<Mutex<Option<ViewData>>> = Arc::new(Mutex::new(None));
    let selection_store: Arc<Mutex<Option<BlockSelection>>> = Arc::new(Mutex::new(None));
    let pending_apply_store: Arc<Mutex<Option<BlockSelection>>> = Arc::new(Mutex::new(None));
    let path_c_store: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(path_c.clone()));
    let apply_history: Arc<Mutex<ApplyHistory>> = Arc::new(Mutex::new(ApplyHistory::new()));

    match (&cli, difft.lock().unwrap().as_ref()) {
        (Err(_), _) => {}
        (_, None) => {
            ui.set_status_text("difft not found.".into());
            ui.set_file_info(install_message().into());
        }
        (Ok(_), Some(path)) => {
            ui.set_status_text(format!("Using difft: {}", path.display()).into());
        }
    }

    {
        let ui_handle = ui.as_weak();
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        let pending_apply_store = Arc::clone(&pending_apply_store);
        ui.on_gutter_clicked(move |row, lhs_side| {
            if let Some(ui) = ui_handle.upgrade() {
                handle_gutter_click(
                    &ui,
                    row,
                    lhs_side,
                    &view_store,
                    &selection_store,
                    &pending_apply_store,
                );
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        let pending_apply_store = Arc::clone(&pending_apply_store);
        ui.on_apply_clicked(move |row, lhs_side| {
            if let Some(ui) = ui_handle.upgrade() {
                handle_apply_click(
                    &ui,
                    row,
                    lhs_side,
                    &view_store,
                    &selection_store,
                    &pending_apply_store,
                );
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        let pending_apply_store = Arc::clone(&pending_apply_store);
        let path_c_store = Arc::clone(&path_c_store);
        let apply_history = Arc::clone(&apply_history);
        let difft_store = Arc::clone(&difft);
        ui.on_center_gutter_clicked(move |row| {
            if let Some(ui) = ui_handle.upgrade() {
                handle_center_gutter_click(
                    &ui,
                    row,
                    &view_store,
                    &selection_store,
                    &pending_apply_store,
                    &path_c_store,
                    &apply_history,
                    &difft_store,
                );
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        let path_c_store = Arc::clone(&path_c_store);
        let apply_history = Arc::clone(&apply_history);
        let difft_store = Arc::clone(&difft);
        ui.on_center_delete_clicked(move |row| {
            if let Some(ui) = ui_handle.upgrade() {
                handle_center_delete_click(
                    &ui,
                    row,
                    &view_store,
                    &selection_store,
                    &path_c_store,
                    &apply_history,
                    &difft_store,
                );
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        let pending_apply_store = Arc::clone(&pending_apply_store);
        ui.on_center_move_clicked(move |row| {
            if let Some(ui) = ui_handle.upgrade() {
                handle_center_move_click(
                    &ui,
                    row,
                    &view_store,
                    &selection_store,
                    &pending_apply_store,
                );
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        let pending_apply_store = Arc::clone(&pending_apply_store);
        ui.on_apply_cancel_requested(move || {
            if let Some(ui) = ui_handle.upgrade() {
                handle_apply_cancel(
                    &ui,
                    &view_store,
                    &selection_store,
                    &pending_apply_store,
                );
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        let pending_apply_store = Arc::clone(&pending_apply_store);
        let path_c_store = Arc::clone(&path_c_store);
        let apply_history = Arc::clone(&apply_history);
        let difft_store = Arc::clone(&difft);
        ui.on_apply_undo_requested(move || {
            if let Some(ui) = ui_handle.upgrade() {
                handle_apply_undo(
                    &ui,
                    &view_store,
                    &selection_store,
                    &pending_apply_store,
                    &path_c_store,
                    &apply_history,
                    &difft_store,
                );
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        let pending_apply_store = Arc::clone(&pending_apply_store);
        let path_c_store = Arc::clone(&path_c_store);
        let apply_history = Arc::clone(&apply_history);
        ui.on_quit_requested(move || {
            if let Some(ui) = ui_handle.upgrade() {
                handle_quit_request(
                    &ui,
                    &view_store,
                    &selection_store,
                    &pending_apply_store,
                    &path_c_store,
                    &apply_history,
                );
            }
        });
    }

    if let (Ok(_), Some(path_a), Some(path_b), true) = (
        &cli,
        path_a,
        path_b,
        difft.lock().unwrap().is_some(),
    ) {
        let ui_handle = ui.as_weak();
        let difft_store = Arc::clone(&difft);
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        let pending_apply_store = Arc::clone(&pending_apply_store);
        let apply_history = Arc::clone(&apply_history);
        slint::Timer::single_shot(Duration::from_millis(0), move || {
            run_diff(
                ui_handle,
                difft_store,
                path_a,
                path_b,
                path_c,
                triple_pane,
                view_store,
                selection_store,
                pending_apply_store,
                apply_history,
            );
        });
    }

    ui.run()
}

#[cfg(test)]
mod apply_tests {
    use super::*;

    #[test]
    fn insert_block_pushes_existing_lines_down() {
        let mut file_c = vec![
            "a".to_owned(),
            "b".to_owned(),
            "c".to_owned(),
        ];
        let diff = DiffFile {
            path: String::new(),
            language: String::new(),
            status: model::DiffStatus::Changed,
            extra_info: None,
            aligned_lines: vec![AlignedLine {
                lhs_line: Some(0),
                rhs_line: Some(0),
                lhs_text: "x".into(),
                rhs_text: "x".into(),
                is_novel_lhs: true,
                is_novel_rhs: true,
                lhs_spans: vec![],
                rhs_spans: vec![],
            }],
            lhs_syntax_blocks: vec![],
            rhs_syntax_blocks: vec![],
        };
        let sel = BlockSelection {
            side: DiffSide::Lhs,
            block_id: 0,
            start_line: 0,
            end_line: 0,
        };

        apply_block_to_file_c(&mut file_c, &diff, sel, 1).unwrap();
        assert_eq!(
            file_c,
            vec![
                "a".to_owned(),
                "x".to_owned(),
                "b".to_owned(),
                "c".to_owned(),
            ]
        );
    }

    #[test]
    fn insert_block_pads_to_target_line_past_eof() {
        let mut file_c = vec!["only".to_owned()];
        let diff = DiffFile {
            path: String::new(),
            language: String::new(),
            status: model::DiffStatus::Changed,
            extra_info: None,
            aligned_lines: vec![AlignedLine {
                lhs_line: Some(0),
                rhs_line: Some(0),
                lhs_text: "new".into(),
                rhs_text: "new".into(),
                is_novel_lhs: true,
                is_novel_rhs: true,
                lhs_spans: vec![],
                rhs_spans: vec![],
            }],
            lhs_syntax_blocks: vec![],
            rhs_syntax_blocks: vec![],
        };
        let sel = BlockSelection {
            side: DiffSide::Lhs,
            block_id: 0,
            start_line: 0,
            end_line: 0,
        };

        apply_block_to_file_c(&mut file_c, &diff, sel, 4).unwrap();
        assert_eq!(file_c.len(), 5);
        assert_eq!(file_c[0], "only");
        assert_eq!(file_c[1], "");
        assert_eq!(file_c[2], "");
        assert_eq!(file_c[3], "");
        assert_eq!(file_c[4], "new");
    }

    #[test]
    fn insert_at_exact_line_not_collapsed_by_leading_empty_gap() {
        let mut file_c = vec!["only".to_owned()];
        let diff = DiffFile {
            path: String::new(),
            language: String::new(),
            status: model::DiffStatus::Changed,
            extra_info: None,
            aligned_lines: vec![AlignedLine {
                lhs_line: Some(0),
                rhs_line: Some(0),
                lhs_text: "x".into(),
                rhs_text: "x".into(),
                is_novel_lhs: true,
                is_novel_rhs: true,
                lhs_spans: vec![],
                rhs_spans: vec![],
            }],
            lhs_syntax_blocks: vec![],
            rhs_syntax_blocks: vec![],
        };
        let sel = BlockSelection {
            side: DiffSide::Lhs,
            block_id: 0,
            start_line: 0,
            end_line: 0,
        };

        apply_block_to_file_c(&mut file_c, &diff, sel, 4).unwrap();
        assert_eq!(file_c[4], "x");
        assert_eq!(file_c[1], "");
        assert_eq!(file_c[2], "");
        assert_eq!(file_c[3], "");
    }

    #[test]
    fn insert_block_overwrites_empty_lines() {
        let mut file_c = vec![
            "a".to_owned(),
            String::new(),
            String::new(),
            "c".to_owned(),
        ];
        let diff = DiffFile {
            path: String::new(),
            language: String::new(),
            status: model::DiffStatus::Changed,
            extra_info: None,
            aligned_lines: vec![
                AlignedLine {
                    lhs_line: Some(0),
                    rhs_line: Some(0),
                    lhs_text: "x".into(),
                    rhs_text: "x".into(),
                    is_novel_lhs: true,
                    is_novel_rhs: true,
                    lhs_spans: vec![],
                    rhs_spans: vec![],
                },
                AlignedLine {
                    lhs_line: Some(1),
                    rhs_line: Some(1),
                    lhs_text: "y".into(),
                    rhs_text: "y".into(),
                    is_novel_lhs: true,
                    is_novel_rhs: true,
                    lhs_spans: vec![],
                    rhs_spans: vec![],
                },
            ],
            lhs_syntax_blocks: vec![],
            rhs_syntax_blocks: vec![],
        };
        let sel = BlockSelection {
            side: DiffSide::Lhs,
            block_id: 0,
            start_line: 0,
            end_line: 1,
        };

        apply_block_to_file_c(&mut file_c, &diff, sel, 1).unwrap();
        assert_eq!(
            file_c,
            vec![
                "a".to_owned(),
                "x".to_owned(),
                "y".to_owned(),
                "c".to_owned(),
            ]
        );
    }

    #[test]
    fn insert_block_displaced_content_overwrites_empty_slots() {
        let mut file_c = vec![
            "a".to_owned(),
            "b".to_owned(),
            String::new(),
            "c".to_owned(),
        ];
        let diff = DiffFile {
            path: String::new(),
            language: String::new(),
            status: model::DiffStatus::Changed,
            extra_info: None,
            aligned_lines: vec![
                AlignedLine {
                    lhs_line: Some(0),
                    rhs_line: Some(0),
                    lhs_text: "x".into(),
                    rhs_text: "x".into(),
                    is_novel_lhs: true,
                    is_novel_rhs: true,
                    lhs_spans: vec![],
                    rhs_spans: vec![],
                },
                AlignedLine {
                    lhs_line: Some(1),
                    rhs_line: Some(1),
                    lhs_text: "y".into(),
                    rhs_text: "y".into(),
                    is_novel_lhs: true,
                    is_novel_rhs: true,
                    lhs_spans: vec![],
                    rhs_spans: vec![],
                },
            ],
            lhs_syntax_blocks: vec![],
            rhs_syntax_blocks: vec![],
        };
        let sel = BlockSelection {
            side: DiffSide::Lhs,
            block_id: 0,
            start_line: 0,
            end_line: 1,
        };

        apply_block_to_file_c(&mut file_c, &diff, sel, 1).unwrap();
        assert_eq!(
            file_c,
            vec![
                "a".to_owned(),
                "x".to_owned(),
                "y".to_owned(),
                "b".to_owned(),
                "c".to_owned(),
            ]
        );
    }

    #[test]
    fn delete_block_clears_selected_lines() {
        let mut file_c = vec![
            "a".to_owned(),
            "block".to_owned(),
            "end".to_owned(),
            "tail".to_owned(),
        ];
        delete_block_from_file_c(&mut file_c, 1, 2).unwrap();
        assert_eq!(
            file_c,
            vec![
                "a".to_owned(),
                String::new(),
                String::new(),
                "tail".to_owned(),
            ]
        );
    }

    #[test]
    fn move_block_reinserts_at_target_line() {
        let mut file_c = vec![
            "a".to_owned(),
            "move-me".to_owned(),
            "b".to_owned(),
            "c".to_owned(),
        ];
        move_block_in_file_c(&mut file_c, 1, 1, 3).unwrap();
        assert_eq!(
            file_c,
            vec![
                "a".to_owned(),
                String::new(),
                "b".to_owned(),
                "move-me".to_owned(),
                "c".to_owned(),
            ]
        );
    }
}
