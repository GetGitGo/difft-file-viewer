//! Build syntax-highlighted line segments from difft JSON spans.

use serde::Deserialize;
use slint::{Brush, Color, SharedString};

pub const TAB_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Highlight {
    Delimiter,
    Normal,
    String,
    Type,
    Comment,
    Keyword,
    TreeSitterError,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TextSpan {
    pub start: u32,
    pub end: u32,
    pub content: String,
    pub highlight: Highlight,
    #[serde(default)]
    pub is_novel: bool,
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub text: String,
    pub color: &'static str,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

const WHITE: &str = "#f8f8f2";
const RED: &str = "#ff5555";
const GREEN: &str = "#50fa7b";
const MAGENTA: &str = "#ff79c6";
const BLUE: &str = "#6272a4";
const PURPLE: &str = "#bd93f9";
const YELLOW: &str = "#f1fa8c";

fn novel_color(side: Side) -> &'static str {
    match side {
        Side::Left => RED,
        Side::Right => GREEN,
    }
}

fn syntax_color(highlight: Highlight) -> &'static str {
    match highlight {
        Highlight::String => MAGENTA,
        Highlight::Comment => BLUE,
        Highlight::TreeSitterError => PURPLE,
        Highlight::Type => YELLOW,
        Highlight::Keyword | Highlight::Delimiter | Highlight::Normal => WHITE,
    }
}

fn style_segment(content: &str, highlight: Highlight, is_novel: bool, side: Side) -> Segment {
    let bold = matches!(highlight, Highlight::Keyword | Highlight::Type);
    let italic = matches!(highlight, Highlight::Comment);
    let color = if is_novel {
        novel_color(side)
    } else {
        syntax_color(highlight)
    };
    Segment {
        text: content.to_owned(),
        color,
        bold,
        italic,
    }
}

fn gap_segment(content: &str, line_novel: bool, side: Side) -> Segment {
    Segment {
        text: content.to_owned(),
        color: if line_novel {
            novel_color(side)
        } else {
            WHITE
        },
        bold: false,
        italic: false,
    }
}

fn merge_segments(mut segments: Vec<Segment>) -> Vec<Segment> {
    if segments.len() < 2 {
        return segments;
    }
    let mut merged: Vec<Segment> = Vec::with_capacity(segments.len());
    for seg in segments.drain(..) {
        if let Some(last) = merged.last_mut() {
            if last.color == seg.color && last.bold == seg.bold && last.italic == seg.italic {
                last.text.push_str(&seg.text);
                continue;
            }
        }
        merged.push(seg);
    }
    merged
}

/// Split a line into colored segments using difft span metadata.
pub fn build_segments(
    text: &str,
    spans: &[TextSpan],
    line_novel: bool,
    side: Side,
) -> Vec<Segment> {
    if text.is_empty() {
        return vec![];
    }
    if spans.is_empty() {
        return vec![gap_segment(text, line_novel, side)];
    }

    let mut sorted: Vec<&TextSpan> = spans.iter().collect();
    sorted.sort_by_key(|span| span.start);

    let mut segments = Vec::new();
    let mut pos = 0usize;

    for span in sorted {
        let start = span.start as usize;
        let end = span.end as usize;
        if start > pos && start <= text.len() {
            segments.push(gap_segment(&text[pos..start], line_novel, side));
        }
        if start < text.len() {
            let content = if span.content.is_empty() {
                &text[start..end.min(text.len())]
            } else {
                span.content.as_str()
            };
            if !content.is_empty() {
                segments.push(style_segment(
                    content,
                    span.highlight,
                    span.is_novel,
                    side,
                ));
            }
        }
        pos = end.max(pos);
    }

    if pos < text.len() {
        segments.push(gap_segment(&text[pos..], line_novel, side));
    }

    merge_segments(segments)
}

/// Expand tabs to spaces for display. Slint renders `\t` as missing-glyph boxes.
pub fn expand_tabs_display(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0usize;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = TAB_WIDTH - (col % TAB_WIDTH);
            out.extend(std::iter::repeat_n(' ', spaces));
            col += spaces;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

fn orig_index_to_display_col(line: &str, index: usize) -> u32 {
    let mut col = 0u32;
    for (i, ch) in line.chars().enumerate() {
        if i >= index {
            break;
        }
        if ch == '\t' {
            col += (TAB_WIDTH - (col as usize % TAB_WIDTH)) as u32;
        } else {
            col += 1;
        }
    }
    col
}

/// Remap span columns after tab expansion and refresh span text slices.
pub fn remap_spans_for_tabs(line: &str, spans: &[TextSpan]) -> Vec<TextSpan> {
    let display = expand_tabs_display(line);
    spans
        .iter()
        .map(|span| {
            let start = orig_index_to_display_col(line, span.start as usize);
            let end = orig_index_to_display_col(line, span.end as usize);
            let start_usize = start as usize;
            let end_usize = end as usize;
            let content = if start_usize < display.len() {
                display[start_usize..end_usize.min(display.len())].to_owned()
            } else {
                span.content.clone()
            };
            TextSpan {
                start,
                end,
                content,
                ..span.clone()
            }
        })
        .collect()
}

pub fn prepare_display_line(line: &str, spans: &[TextSpan]) -> (String, Vec<TextSpan>) {
    if !line.contains('\t') {
        return (line.to_owned(), spans.to_vec());
    }
    let display = expand_tabs_display(line);
    let spans = remap_spans_for_tabs(line, spans);
    (display, spans)
}

fn brush_from_hex(hex: &str) -> Brush {
    let hex = hex.trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).unwrap_or(0xf8f8f2);
    Brush::from(Color::from_rgb_u8(
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ))
}

/// Approximate advance width for "Courier New" 12px in the viewer.
pub const CHAR_WIDTH: f32 = 7.85;

pub const GUTTER_LINE: &str = "#6272a4";
pub const GUTTER_SELECTED: &str = "#bd93f9";
pub const GUTTER_INSERT: &str = "#50fa7b";

pub fn adjust_brightness_hex(hex: &str, factor: f32) -> String {
    let hex = hex.trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).unwrap_or(0xf8_f8_f2);
    let r = ((value >> 16) & 0xff) as f32;
    let g = ((value >> 8) & 0xff) as f32;
    let b = (value & 0xff) as f32;
    let scale = |c: f32| (c * factor).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", scale(r), scale(g), scale(b))
}

pub fn brush_with_brightness(hex: &str, factor: f32) -> Brush {
    brush_from_hex(&adjust_brightness_hex(hex, factor))
}

/// Global code foreground scale (fixed; max supported by the viewer).
pub const CODE_BRIGHTNESS: f32 = 1.3;

pub fn code_brush(hex: &str) -> Brush {
    brush_with_brightness(hex, CODE_BRIGHTNESS)
}

pub fn plain_line_brush(novel: bool, side: Side) -> Brush {
    let hex = if novel {
        novel_color(side)
    } else {
        WHITE
    };
    code_brush(hex)
}

pub fn text_pixel_width(text: &str) -> f32 {
    expand_tabs_display(text).chars().count() as f32 * CHAR_WIDTH
}

pub fn to_slint_segments(segments: &[Segment]) -> slint::ModelRc<crate::TextSegment> {
    let mut x = 0.0f32;
    slint::ModelRc::new(slint::VecModel::from(
        segments
            .iter()
            .map(|seg| {
                let item = crate::TextSegment {
                    text: SharedString::from(seg.text.as_str()),
                    color: code_brush(seg.color),
                    bold: seg.bold,
                    italic: seg.italic,
                    x_offset: x,
                };
                x += seg.text.chars().count() as f32 * CHAR_WIDTH;
                item
            })
            .collect::<Vec<_>>(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_string_uses_magenta() {
        let spans = vec![TextSpan {
            start: 6,
            end: 18,
            content: "\"Does stuff.\"".into(),
            highlight: Highlight::String,
            is_novel: false,
        }];
        let segments = build_segments("      \"Does stuff.\"", &spans, false, Side::Left);
        assert!(segments.iter().any(|s| s.color == MAGENTA));
    }

    #[test]
    fn novel_string_uses_side_color() {
        let spans = vec![TextSpan {
            start: 11,
            end: 19,
            content: "\"hello!\"".into(),
            highlight: Highlight::String,
            is_novel: true,
        }];
        let segments = build_segments("  (println \"hello!\")", &spans, true, Side::Left);
        assert!(segments.iter().any(|s| s.color == RED && s.text.contains("hello")));
    }

    #[test]
    fn adjust_brightness_scales_rgb() {
        assert_eq!(adjust_brightness_hex("#808080", 1.5), "#c0c0c0");
        assert_eq!(adjust_brightness_hex("#ffffff", 0.5), "#808080");
    }

    #[test]
    fn expand_tabs_replaces_leading_indent() {
        assert_eq!(expand_tabs_display("\tif (x)"), "    if (x)");
        assert_eq!(expand_tabs_display("\t\treturn;"), "        return;");
    }

    #[test]
    fn remap_spans_after_tab_expansion() {
        let line = "\tif x";
        let spans = vec![TextSpan {
            start: 1,
            end: 3,
            content: "if".into(),
            highlight: Highlight::Keyword,
            is_novel: false,
        }];
        let (display, remapped) = prepare_display_line(line, &spans);
        assert_eq!(display, "    if x");
        assert_eq!(remapped[0].start, 4);
        assert_eq!(remapped[0].end, 6);
        assert_eq!(remapped[0].content, "if");
    }

    #[test]
    fn text_pixel_width_counts_expanded_tabs() {
        assert_eq!(text_pixel_width("\t\t"), 8.0 * CHAR_WIDTH);
    }

    #[test]
    fn adjust_brightness_clamps_to_byte_range() {
        assert_eq!(adjust_brightness_hex("#ffffff", 2.0), "#ffffff");
        assert_eq!(adjust_brightness_hex("#000000", 0.1), "#000000");
    }
}
