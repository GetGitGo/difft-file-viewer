//! JSON types matching `difft --display json` (GuiDiffFile schema).

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
    pub lhs_text: String,
    pub rhs_text: String,
    pub is_novel_lhs: bool,
    pub is_novel_rhs: bool,
    #[serde(default)]
    pub lhs_spans: Vec<crate::segments::TextSpan>,
    #[serde(default)]
    pub rhs_spans: Vec<crate::segments::TextSpan>,
}

pub fn parse_diff_json(stdout: &[u8]) -> Result<DiffFile, String> {
    let trimmed = std::str::from_utf8(stdout)
        .map_err(|e| e.to_string())?
        .trim();
    if trimmed.is_empty() {
        return Err("difft produced no JSON output.".to_owned());
    }

    if trimmed.starts_with('[') {
        let files: Vec<DiffFile> =
            serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON array: {e}"))?;
        files
            .into_iter()
            .next()
            .ok_or_else(|| "difft returned an empty JSON array.".to_owned())
    } else {
        serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON: {e}"))
    }
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

/// Smallest non-trivial syntax block containing `line` (0-based file line index).
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
