//! JSON types matching `difft --display json`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::line_ending::{normalize_line, split_logical_lines};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SyntaxBlock {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub label: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiffFile {
    #[allow(dead_code)]
    pub path: String,
    #[allow(dead_code)]
    pub language: String,
    #[allow(dead_code)]
    pub status: DiffStatus,
    #[allow(dead_code)]
    pub extra_info: Option<String>,
    pub aligned_lines: Vec<AlignedLine>,
    #[serde(default)]
    pub lhs_syntax_blocks: Vec<SyntaxBlock>,
    #[serde(default)]
    pub rhs_syntax_blocks: Vec<SyntaxBlock>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum DiffStatus {
    Unchanged,
    Changed,
    Created,
    Deleted,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlignedLine {
    pub lhs_line: Option<u32>,
    pub rhs_line: Option<u32>,
    /// File C line index when this row is not tied to an A/B alignment (trailing C-only rows).
    #[serde(default)]
    pub file_c_only_line: Option<u32>,
    pub lhs_text: String,
    pub rhs_text: String,
    pub is_novel_lhs: bool,
    pub is_novel_rhs: bool,
    #[serde(default)]
    pub lhs_spans: Vec<crate::segments::TextSpan>,
    #[serde(default)]
    pub rhs_spans: Vec<crate::segments::TextSpan>,
}

#[derive(Debug, Deserialize)]
struct NewDiffFile {
    path: String,
    language: String,
    status: DiffStatus,
    #[serde(default)]
    extra_info: Option<String>,
    #[serde(default)]
    aligned_lines: Vec<(Option<u32>, Option<u32>)>,
    #[serde(default)]
    chunks: Vec<Vec<NewChunkLine>>,
    #[serde(default)]
    lhs_syntax_blocks: Vec<SyntaxBlock>,
    #[serde(default)]
    rhs_syntax_blocks: Vec<SyntaxBlock>,
}

#[derive(Debug, Deserialize)]
struct NewChunkLine {
    #[serde(default)]
    lhs: Option<NewSide>,
    #[serde(default)]
    rhs: Option<NewSide>,
}

#[derive(Debug, Deserialize)]
struct NewSide {
    line_number: u32,
    #[serde(default)]
    changes: Vec<NewChange>,
}

#[derive(Debug, Deserialize)]
struct NewChange {
    start: u32,
    end: u32,
    content: String,
    highlight: crate::segments::Highlight,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SingleFileSyntaxBlocks {
    #[allow(dead_code)]
    pub path: String,
    #[allow(dead_code)]
    pub language: String,
    pub syntax_blocks: Vec<SyntaxBlock>,
}

pub fn parse_syntax_blocks_json(stdout: &[u8]) -> Result<SingleFileSyntaxBlocks, String> {
    let trimmed = std::str::from_utf8(stdout)
        .map_err(|e| e.to_string())?
        .trim();
    if trimmed.is_empty() {
        return Err("difft produced no syntax blocks JSON.".to_owned());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("invalid syntax blocks JSON: {e}"))
}

pub fn parse_diff_json(stdout: &[u8], path_a: &Path, path_b: &Path) -> Result<DiffFile, String> {
    let trimmed = std::str::from_utf8(stdout)
        .map_err(|e| e.to_string())?
        .trim();
    if trimmed.is_empty() {
        return Err("difft produced no JSON output.".to_owned());
    }

    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("invalid JSON: {e}"))?;
    let value = if let Some(array) = value.as_array() {
        array
            .first()
            .cloned()
            .ok_or_else(|| "difft returned an empty JSON array.".to_owned())?
    } else {
        value
    };

    if uses_legacy_aligned_lines(&value) {
        let mut file: DiffFile =
            serde_json::from_value(value).map_err(|e| format!("invalid JSON: {e}"))?;
        normalize_diff_file(&mut file);
        Ok(file)
    } else {
        let raw: NewDiffFile =
            serde_json::from_value(value).map_err(|e| format!("invalid JSON: {e}"))?;
        convert_new_diff_json(raw, path_a, path_b)
    }
}

fn uses_legacy_aligned_lines(value: &serde_json::Value) -> bool {
    value
        .get("aligned_lines")
        .and_then(|lines| lines.as_array())
        .is_some_and(|lines| lines.first().is_some_and(|line| line.is_object()))
}

fn read_file_lines(path: &Path) -> Result<Vec<String>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    Ok(split_logical_lines(&content))
}

/// Inclusive 0-based line range from a source file (for Apply/Move from A or B).
pub fn file_lines_in_range(path: &Path, start: u32, end: u32) -> Result<Vec<String>, String> {
    let lines = read_file_lines(path)?;
    let start = start as usize;
    if start >= lines.len() {
        return Ok(vec![]);
    }
    let end = (end as usize).min(lines.len() - 1);
    if start > end {
        return Ok(vec![]);
    }
    Ok(lines[start..=end].to_vec())
}

fn normalize_diff_file(file: &mut DiffFile) {
    for line in &mut file.aligned_lines {
        line.lhs_text = normalize_line(&line.lhs_text);
        line.rhs_text = normalize_line(&line.rhs_text);
        for span in &mut line.lhs_spans {
            span.content = normalize_line(&span.content);
        }
        for span in &mut line.rhs_spans {
            span.content = normalize_line(&span.content);
        }
    }
}

fn line_text(lines: &[String], line: Option<u32>) -> String {
    line.and_then(|n| lines.get(n as usize))
        .cloned()
        .unwrap_or_default()
}

fn changes_to_spans(changes: &[NewChange]) -> Vec<crate::segments::TextSpan> {
    changes
        .iter()
        .map(|change| crate::segments::TextSpan {
            start: change.start,
            end: change.end,
            content: normalize_line(&change.content),
            highlight: change.highlight,
            is_novel: true,
        })
        .collect()
}

fn aligned_pairs_for_status(
    status: DiffStatus,
    lhs_lines: &[String],
    rhs_lines: &[String],
) -> Vec<(Option<u32>, Option<u32>)> {
    match status {
        DiffStatus::Unchanged => {
            let count = lhs_lines.len().max(rhs_lines.len());
            (0..count as u32)
                .map(|line| (Some(line), Some(line)))
                .collect()
        }
        DiffStatus::Created => (0..rhs_lines.len() as u32)
            .map(|line| (None, Some(line)))
            .collect(),
        DiffStatus::Deleted => (0..lhs_lines.len() as u32)
            .map(|line| (Some(line), None))
            .collect(),
        DiffStatus::Changed => Vec::new(),
    }
}

fn convert_new_diff_json(raw: NewDiffFile, path_a: &Path, path_b: &Path) -> Result<DiffFile, String> {
    let lhs_lines = read_file_lines(path_a)?;
    let rhs_lines = read_file_lines(path_b)?;

    let pairs = if raw.aligned_lines.is_empty() {
        aligned_pairs_for_status(raw.status, &lhs_lines, &rhs_lines)
    } else {
        raw.aligned_lines
    };

    let mut chunk_map: HashMap<(Option<u32>, Option<u32>), NewChunkLine> = HashMap::new();
    for chunk in raw.chunks {
        for line in chunk {
            let key = (
                line.lhs.as_ref().map(|side| side.line_number),
                line.rhs.as_ref().map(|side| side.line_number),
            );
            chunk_map.insert(key, line);
        }
    }

    let aligned_lines = pairs
        .into_iter()
        .map(|(lhs_line, rhs_line)| {
            let chunk = chunk_map.get(&(lhs_line, rhs_line));
            let lhs_spans = chunk
                .and_then(|line| line.lhs.as_ref())
                .map(|side| changes_to_spans(&side.changes))
                .unwrap_or_default();
            let rhs_spans = chunk
                .and_then(|line| line.rhs.as_ref())
                .map(|side| changes_to_spans(&side.changes))
                .unwrap_or_default();
            let is_novel_lhs = match chunk {
                Some(line) => {
                    line.lhs
                        .as_ref()
                        .is_some_and(|side| !side.changes.is_empty())
                        || rhs_line.is_none()
                }
                None => false,
            };
            let is_novel_rhs = match chunk {
                Some(line) => {
                    line.rhs
                        .as_ref()
                        .is_some_and(|side| !side.changes.is_empty())
                        || lhs_line.is_none()
                }
                None => false,
            };
            AlignedLine {
                lhs_line,
                rhs_line,
                file_c_only_line: None,
                lhs_text: line_text(&lhs_lines, lhs_line),
                rhs_text: line_text(&rhs_lines, rhs_line),
                is_novel_lhs,
                is_novel_rhs,
                lhs_spans,
                rhs_spans,
            }
        })
        .collect();

    let mut file = DiffFile {
        path: raw.path,
        language: raw.language,
        status: raw.status,
        extra_info: raw.extra_info,
        aligned_lines,
        lhs_syntax_blocks: raw.lhs_syntax_blocks,
        rhs_syntax_blocks: raw.rhs_syntax_blocks,
    };
    normalize_diff_file(&mut file);
    Ok(file)
}

fn is_trivial_syntax_block(block: &SyntaxBlock) -> bool {
    block.start_line == block.end_line && block.label.len() <= 3
}

fn block_label_priority(label: &str) -> u32 {
    if label.starts_with("(if")
        || label.starts_with("(fn")
        || label.starts_with("(for")
        || label.starts_with("(while")
        || label.starts_with("(match")
        || label.starts_with("(loop")
        || label.starts_with("(def")
        || label.starts_with("(class")
    {
        0
    } else if label.starts_with('(') && !label.starts_with("({") {
        1
    } else if label.starts_with('{') {
        3
    } else {
        2
    }
}

/// Syntax block to highlight when the user clicks a line-number gutter.
pub fn gutter_syntax_block(blocks: &[SyntaxBlock], line: u32) -> Option<&SyntaxBlock> {
    let containing: Vec<&SyntaxBlock> = blocks
        .iter()
        .filter(|b| {
            b.start_line <= line && line <= b.end_line && !is_trivial_syntax_block(b)
        })
        .collect();

    if containing.is_empty() {
        return innermost_syntax_block(blocks, line);
    }

    if let Some(block) = containing
        .iter()
        .filter(|b| is_statement_label(&b.label))
        .min_by_key(|b| block_span_size(b))
    {
        return Some(block);
    }

    if let Some(block) = containing
        .iter()
        .filter(|b| is_declaration_label(&b.label))
        .min_by_key(|b| block_span_size(b))
    {
        return Some(block);
    }

    innermost_syntax_block(blocks, line)
}

fn block_span_size(block: &SyntaxBlock) -> u32 {
    block.end_line.saturating_sub(block.start_line)
}

fn is_statement_label(label: &str) -> bool {
    label.starts_with("(match arm")
        || label.starts_with("(if")
        || label.starts_with("(match")
        || label.starts_with("(for")
        || label.starts_with("(while")
        || label.starts_with("(loop")
        || label.starts_with("(switch")
}

fn is_declaration_label(label: &str) -> bool {
    label.starts_with("(fn")
        || label.starts_with("(impl")
        || label.starts_with("(struct")
        || label.starts_with("(class")
        || label.starts_with("(enum")
        || label.starts_with("(mod")
        || label.starts_with("(trait")
        || label.starts_with("(def")
}

/// Smallest non-trivial syntax block containing `line` (0-based file index).
pub fn innermost_syntax_block(blocks: &[SyntaxBlock], line: u32) -> Option<&SyntaxBlock> {
    let mut best: Option<&SyntaxBlock> = None;
    for block in blocks
        .iter()
        .filter(|b| b.start_line <= line && line <= b.end_line)
    {
        let replace = match best {
            None => true,
            Some(prev) => {
                let prev_trivial = is_trivial_syntax_block(prev);
                let block_trivial = is_trivial_syntax_block(block);
                if prev_trivial != block_trivial {
                    prev_trivial
                } else {
                    let size_prev = prev.end_line.saturating_sub(prev.start_line);
                    let size_block = block.end_line.saturating_sub(block.start_line);
                    size_block < size_prev
                        || (size_block == size_prev
                            && block_label_priority(&block.label)
                                < block_label_priority(&prev.label))
                }
            }
        };
        if replace {
            best = Some(block);
        }
    }
    best.or_else(|| {
        blocks
            .iter()
            .find(|b| b.start_line <= line && line <= b.end_line)
    })
}

/// Warning text when diff succeeded but fell back (parse errors, byte limit, etc.).
pub fn warning_message(file: &DiffFile) -> Option<String> {
    if let Some(info) = &file.extra_info {
        if !info.is_empty() {
            return Some(info.clone());
        }
    }
    if file.language.starts_with("Text (") {
        return Some(file.language.clone());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_file(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("difft-viewer-{name}-{}.txt", std::process::id()));
        let mut file = fs::File::create(&path).unwrap();
        write!(file, "{content}").unwrap();
        path
    }

    #[test]
    fn parse_new_json_format_reads_source_lines() {
        let path_a = write_temp_file("a", "hello\nkeep");
        let path_b = write_temp_file("b", "world\nkeep");
        let json = br#"{
            "aligned_lines": [[0,0],[1,1]],
            "chunks": [[{
                "lhs": {"line_number": 0, "changes": [{"start": 0, "end": 5, "content": "hello", "highlight": "normal"}]},
                "rhs": {"line_number": 0, "changes": [{"start": 0, "end": 5, "content": "world", "highlight": "normal"}]}
            }]],
            "language": "Text",
            "path": "b",
            "status": "changed"
        }"#;

        let diff = parse_diff_json(json, &path_a, &path_b).unwrap();
        assert_eq!(diff.aligned_lines.len(), 2);
        assert_eq!(diff.aligned_lines[0].lhs_text, "hello");
        assert_eq!(diff.aligned_lines[0].rhs_text, "world");
        assert!(diff.aligned_lines[0].is_novel_lhs);
        assert!(diff.aligned_lines[0].is_novel_rhs);
        assert!(!diff.aligned_lines[1].is_novel_lhs);
        assert_eq!(diff.aligned_lines[1].lhs_text, "keep");
    }

    #[test]
    fn parse_unchanged_without_aligned_lines() {
        let path_a = write_temp_file("unchanged-a", "same\nline");
        let path_b = write_temp_file("unchanged-b", "same\nline");
        let json = br#"{"language":"Text","path":"a","status":"unchanged"}"#;

        let diff = parse_diff_json(json, &path_a, &path_b).unwrap();
        assert_eq!(diff.aligned_lines.len(), 2);
        assert_eq!(diff.aligned_lines[0].lhs_text, "same");
        assert!(!diff.aligned_lines[0].is_novel_lhs);
    }

    #[test]
    fn parse_reads_crlf_source_lines() {
        let path_a = write_temp_file("crlf-a", "left\r\n");
        let path_b = write_temp_file("crlf-b", "right\r\n");
        let json = br#"{
            "aligned_lines": [[0,0]],
            "chunks": [[{
                "lhs": {"line_number": 0, "changes": [{"start": 0, "end": 4, "content": "left", "highlight": "normal"}]},
                "rhs": {"line_number": 0, "changes": [{"start": 0, "end": 5, "content": "right", "highlight": "normal"}]}
            }]],
            "language": "Text",
            "path": "b",
            "status": "changed"
        }"#;

        let diff = parse_diff_json(json, &path_a, &path_b).unwrap();
        assert_eq!(diff.aligned_lines[0].lhs_text, "left");
        assert_eq!(diff.aligned_lines[0].rhs_text, "right");
    }

    #[test]
    fn parse_new_json_format_reads_syntax_blocks() {
        let path_a = write_temp_file("syn-a", "fn a() {}\n");
        let path_b = write_temp_file("syn-b", "fn b() {}\n");
        let json = br#"{
            "aligned_lines": [[0,0]],
            "chunks": [],
            "lhs_syntax_blocks": [
                {"id": 1, "parent_id": null, "label": "(fn a", "start_line": 0, "end_line": 0}
            ],
            "rhs_syntax_blocks": [
                {"id": 2, "parent_id": null, "label": "(fn b", "start_line": 0, "end_line": 0}
            ],
            "language": "Rust",
            "path": "b",
            "status": "changed"
        }"#;

        let diff = parse_diff_json(json, &path_a, &path_b).unwrap();
        assert_eq!(diff.lhs_syntax_blocks.len(), 1);
        assert_eq!(diff.lhs_syntax_blocks[0].label, "(fn a");
        assert_eq!(diff.rhs_syntax_blocks[0].start_line, 0);
        assert!(gutter_syntax_block(&diff.lhs_syntax_blocks, 0).is_some());
    }

    #[test]
    fn parse_syntax_blocks_json_reads_dump_output() {
        let json = br#"{
            "path": "foo.c",
            "language": "C++",
            "syntax_blocks": [
                {"id": 37, "parent_id": 31, "label": "(if", "start_line": 10, "end_line": 25}
            ]
        }"#;
        let file = parse_syntax_blocks_json(json).unwrap();
        assert_eq!(file.syntax_blocks.len(), 1);
        assert_eq!(file.syntax_blocks[0].label, "(if");
        assert_eq!(file.syntax_blocks[0].start_line, 10);
    }
}
