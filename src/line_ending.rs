//! LF / CRLF / CR helpers — split like difft, normalize for display, preserve on write.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

/// Prefer CRLF when present, else legacy CR-only, else LF.
pub fn detect_line_ending(content: &str) -> LineEnding {
    if content.contains("\r\n") {
        LineEnding::CrLf
    } else if content.contains('\r') {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    }
}

/// Remove a trailing `\r` from one split segment (CRLF / CR source files).
pub fn normalize_line(line: &str) -> String {
    line.strip_suffix('\r').unwrap_or(line).to_owned()
}

/// Split like difft (`str::split('\n')`) and strip `\r` from each segment.
pub fn split_logical_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return vec![];
    }
    content.split('\n').map(normalize_line).collect()
}

pub fn join_lines(lines: &[String], ending: LineEnding) -> String {
    lines.join(ending.as_str())
}

pub fn read_lines_from_content(content: &str) -> (Vec<String>, LineEnding) {
    (
        split_logical_lines(content),
        detect_line_ending(content),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_splits_like_difft_and_strips_cr() {
        assert_eq!(
            split_logical_lines("a\r\nb\r\n"),
            vec!["a".to_owned(), "b".to_owned(), String::new()]
        );
        assert_eq!(split_logical_lines("a\r\nb"), vec!["a", "b"]);
    }

    #[test]
    fn lf_unchanged() {
        assert_eq!(
            split_logical_lines("a\nb\n"),
            vec!["a".to_owned(), "b".to_owned(), String::new()]
        );
    }

    #[test]
    fn join_roundtrip_crlf() {
        let ending = LineEnding::CrLf;
        let lines = split_logical_lines("x\r\ny\r\n");
        assert_eq!(join_lines(&lines, ending), "x\r\ny\r\n");
    }

    #[test]
    fn detect_prefers_crlf() {
        assert_eq!(detect_line_ending("a\r\nb"), LineEnding::CrLf);
        assert_eq!(detect_line_ending("a\nb"), LineEnding::Lf);
        assert_eq!(detect_line_ending("a\rb"), LineEnding::Cr);
    }
}
