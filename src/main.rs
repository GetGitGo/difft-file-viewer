mod clang_format_preprocess;
mod difft_probe;
mod model;
mod segments;

slint::include_modules!();

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use difft_probe::{difft_command, install_message, probe_difft};
use model::{
    gutter_syntax_block, parse_diff_json, warning_message, AlignedLine, DiffFile, SyntaxBlock,
};
use segments::{build_segments, text_pixel_width, to_slint_segments, Side};

const BYTE_LIMIT: &str = "32000000";
/// Show essentially the whole file in the GUI (not just changed hunks).
const FULL_FILE_CONTEXT: &str = "999999";
const MAX_APPLY_HISTORY: usize = 100;

#[derive(Clone)]
struct ViewData {
    diff: DiffFile,
    file_c_lines: Vec<String>,
    triple_pane: bool,
}

struct CliArgs {
    path_a: PathBuf,
    path_b: PathBuf,
    path_c: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffSide {
    Lhs,
    Rhs,
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
}

fn cli_args() -> CliArgs {
    let paths: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    match paths.len() {
        2 => CliArgs {
            path_a: paths[0].clone(),
            path_b: paths[1].clone(),
            path_c: None,
        },
        3 => CliArgs {
            path_a: paths[0].clone(),
            path_b: paths[1].clone(),
            path_c: Some(paths[2].clone()),
        },
        _ => require_cli_files(paths.len()),
    }
}

fn usage() -> &'static str {
    "Usage: difft-file-viewer <file-a> <file-b> [<file-c>]"
}

fn require_cli_files(got: usize) -> ! {
    eprintln!("{}", usage());
    if got == 0 {
        eprintln!("Error: at least two file paths are required.");
    } else if got == 1 {
        eprintln!("Error: at least two file paths are required (got 1).");
    } else {
        eprintln!("Error: expected 2 or 3 file paths (got {got}).");
    }
    std::process::exit(1);
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

fn apply_block_to_file_c(view: &mut ViewData, sel: BlockSelection) -> Result<(), String> {
    let block_lines = block_source_lines(&view.diff, sel.side, sel.start_line, sel.end_line);
    if block_lines.is_empty() {
        return Err("selected syntax block has no source lines.".to_owned());
    }

    let needed_len = sel.end_line as usize + 1;
    if view.file_c_lines.len() < needed_len {
        view.file_c_lines.resize(needed_len, String::new());
    }

    for (line_no, text) in block_lines {
        view.file_c_lines[line_no as usize] = text;
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
        return parse_diff_json(&output.stdout);
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

fn slint_line(line: &AlignedLine, view: &ViewData, sel: Option<BlockSelection>) -> DiffLine {
    let (center_line, center_text) = if view.triple_pane {
        center_line_for_aligned(line, &view.file_c_lines)
    } else {
        (-1, String::new())
    };

    DiffLine {
        lhs_novel: line.is_novel_lhs,
        rhs_novel: line.is_novel_rhs,
        lhs_line: line_num(line.lhs_line),
        rhs_line: line_num(line.rhs_line),
        center_line,
        lhs_selected: line_in_selection(line.lhs_line, sel, DiffSide::Lhs),
        rhs_selected: line_in_selection(line.rhs_line, sel, DiffSide::Rhs),
        center_selected: false,
        lhs_show_apply: show_apply_on_line(view.triple_pane, sel, DiffSide::Lhs, line.lhs_line),
        rhs_show_apply: show_apply_on_line(view.triple_pane, sel, DiffSide::Rhs, line.rhs_line),
        lhs_plain_text: line.lhs_text.clone().into(),
        rhs_plain_text: line.rhs_text.clone().into(),
        center_plain_text: center_text.clone().into(),
        lhs_segments: to_slint_segments(&build_segments(
            &line.lhs_text,
            &line.lhs_spans,
            line.is_novel_lhs,
            Side::Left,
        )),
        rhs_segments: to_slint_segments(&build_segments(
            &line.rhs_text,
            &line.rhs_spans,
            line.is_novel_rhs,
            Side::Right,
        )),
        center_segments: to_slint_segments(&build_segments(&center_text, &[], false, Side::Left)),
        lhs_content_width: text_pixel_width(&line.lhs_text),
        rhs_content_width: text_pixel_width(&line.rhs_text),
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

fn set_lines_on_ui(ui: &MainWindow, view: &ViewData, sel: Option<BlockSelection>) {
    let lines = slint_lines(view, sel);
    ui.set_max_content_width(max_line_content_width(&lines));
    let model: slint::ModelRc<DiffLine> = std::rc::Rc::new(slint::VecModel::from(lines)).into();
    ui.set_lines(model);
}

fn slint_lines(view: &ViewData, sel: Option<BlockSelection>) -> Vec<DiffLine> {
    view.diff
        .aligned_lines
        .iter()
        .map(|line| slint_line(line, view, sel))
        .collect()
}

fn blocks_for_side<'a>(file: &'a DiffFile, side: DiffSide) -> &'a [SyntaxBlock] {
    match side {
        DiffSide::Lhs => &file.lhs_syntax_blocks,
        DiffSide::Rhs => &file.rhs_syntax_blocks,
    }
}

fn handle_gutter_click(
    ui: &MainWindow,
    row: i32,
    lhs_side: bool,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
) {
    if row < 0 {
        *selection_store.lock().unwrap() = None;
        if let Some(view) = view_store.lock().unwrap().clone() {
            set_lines_on_ui(ui, &view, None);
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
        set_lines_on_ui(ui, &view, None);
        return;
    }

    let new_sel = BlockSelection {
        side,
        block_id: block.id,
        start_line: block.start_line,
        end_line: block.end_line,
    };
    *selection = Some(new_sel);
    set_lines_on_ui(ui, &view, Some(new_sel));
}

fn handle_apply_click(
    ui: &MainWindow,
    row: i32,
    lhs_side: bool,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    path_c_store: &Arc<Mutex<Option<PathBuf>>>,
    apply_history: &Arc<Mutex<ApplyHistory>>,
) {
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
    };
    if block_start != Some(sel.start_line) {
        return;
    }

    let Some(path_c) = path_c_store.lock().unwrap().clone() else {
        ui.set_file_info("Apply requires a file-c path.".into());
        return;
    };

    let mut view = view;
    let snapshot = view.file_c_lines.clone();
    match apply_block_to_file_c(&mut view, sel) {
        Ok(()) => {
            if let Err(err) = write_file_lines(&path_c, &view.file_c_lines) {
                ui.set_file_info(err.into());
                return;
            }
            apply_history.lock().unwrap().push_snapshot(&snapshot);
            *view_store.lock().unwrap() = Some(view.clone());
            set_lines_on_ui(ui, &view, Some(sel));
            ui.set_file_info(format!("Applied to {}.", path_c.display()).into());
        }
        Err(err) => {
            ui.set_file_info(err.into());
        }
    }
}

fn handle_apply_undo(
    ui: &MainWindow,
    view_store: &Arc<Mutex<Option<ViewData>>>,
    selection_store: &Arc<Mutex<Option<BlockSelection>>>,
    path_c_store: &Arc<Mutex<Option<PathBuf>>>,
    apply_history: &Arc<Mutex<ApplyHistory>>,
) {
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

    *view_store.lock().unwrap() = Some(view.clone());
    let sel = *selection_store.lock().unwrap();
    set_lines_on_ui(ui, &view, sel);
    ui.set_file_info(format!("Undid last Apply on {}.", path_c.display()).into());
}

fn set_path_label(ui: &MainWindow, property: fn(&MainWindow, slint::SharedString), path: PathBuf) {
    let path = full_path(path);
    property(ui, path.display().to_string().into());
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
        let mut input_paths = vec![path_a.clone(), path_b.clone()];
        if let Some(path_c) = &path_c {
            input_paths.push(path_c.clone());
        }
        let preprocess_note = clang_format_preprocess::preprocess_input_files(&input_paths);

        let outcome: Result<ViewData, String> = (|| {
            let diff = run_difft(&difft_path, &path_a, &path_b)?;
            let file_c_lines = if triple_pane {
                let path_c = path_c.ok_or_else(|| "internal error: missing file-c path".to_string())?;
                open_or_create_file_lines(&path_c)?
            } else {
                vec![]
            };
            Ok(ViewData {
                diff,
                file_c_lines,
                triple_pane,
            })
        })();

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_handle.upgrade() {
                match outcome {
                    Ok(view) => {
                        *selection_store.lock().unwrap() = None;
                        apply_history.lock().unwrap().clear();
                        *view_store.lock().unwrap() = Some(view.clone());
                        set_lines_on_ui(&ui, &view, None);
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

fn main() -> Result<(), slint::PlatformError> {
    let cli = cli_args();
    let triple_pane = cli.path_c.is_some();

    let ui = MainWindow::new()?;
    maximize_on_startup(&ui);
    ui.set_triple_pane(triple_pane);

    let path_a = full_path(cli.path_a);
    let path_b = full_path(cli.path_b);
    let path_c = cli.path_c.map(full_path);

    set_path_label(&ui, MainWindow::set_path_a, path_a.clone());
    set_path_label(&ui, MainWindow::set_path_b, path_b.clone());
    if let Some(path_c) = &path_c {
        set_path_label(&ui, MainWindow::set_path_c, path_c.clone());
    }

    let difft = Arc::new(Mutex::new(probe_difft().ok()));
    let view_store: Arc<Mutex<Option<ViewData>>> = Arc::new(Mutex::new(None));
    let selection_store: Arc<Mutex<Option<BlockSelection>>> = Arc::new(Mutex::new(None));
    let path_c_store: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(path_c.clone()));
    let apply_history: Arc<Mutex<ApplyHistory>> = Arc::new(Mutex::new(ApplyHistory::new()));

    match difft.lock().unwrap().as_ref() {
        Some(path) => {
            ui.set_status_text(format!("Using difft: {}", path.display()).into());
        }
        None => {
            ui.set_status_text("difft not found.".into());
            ui.set_file_info(install_message().into());
        }
    }

    {
        let ui_handle = ui.as_weak();
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
        ui.on_gutter_clicked(move |row, lhs_side| {
            if let Some(ui) = ui_handle.upgrade() {
                handle_gutter_click(
                    &ui,
                    row,
                    lhs_side,
                    &view_store,
                    &selection_store,
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
        ui.on_apply_clicked(move |row, lhs_side| {
            if let Some(ui) = ui_handle.upgrade() {
                handle_apply_click(
                    &ui,
                    row,
                    lhs_side,
                    &view_store,
                    &selection_store,
                    &path_c_store,
                    &apply_history,
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
        ui.on_apply_undo_requested(move || {
            if let Some(ui) = ui_handle.upgrade() {
                handle_apply_undo(
                    &ui,
                    &view_store,
                    &selection_store,
                    &path_c_store,
                    &apply_history,
                );
            }
        });
    }

    if difft.lock().unwrap().is_some() {
        let ui_handle = ui.as_weak();
        let difft_store = Arc::clone(&difft);
        let view_store = Arc::clone(&view_store);
        let selection_store = Arc::clone(&selection_store);
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
                apply_history,
            );
        });
    }

    ui.run()
}
