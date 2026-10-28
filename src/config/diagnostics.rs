//! Dependency-free compiler-style diagnostics. Source snapshots are taken while
//! decoding: errors stay useful even if an editor replaces an included file.
use super::decode::ValidationIssue;
use super::kdl::Pos;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceLabel {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub source_line: String,
    pub label: String,
}

impl SourceLabel {
    pub fn at(file: &str, source: &str, pos: Pos, label: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line: pos.line as usize,
            column: pos.col as usize,
            source_line: source
                .lines()
                .nth(pos.line.saturating_sub(1) as usize)
                .unwrap_or("")
                .into(),
            label: label.into(),
        }
    }
    fn render(&self) -> String {
        format!(
            "{}:{}:{}: {}\n {:>4} | {}\n      | {}^ {}",
            self.file,
            self.line,
            self.column,
            self.label,
            self.line,
            self.source_line,
            " ".repeat(self.column.saturating_sub(1)),
            self.label
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticContext {
    pub source_line: String,
    pub related: Vec<SourceLabel>,
    pub includes: Vec<SourceLabel>,
    pub help: String,
}

pub fn render(issue: &ValidationIssue, default_file: &str) -> String {
    let mut out = issue.located(default_file);
    if !issue.context.source_line.is_empty() {
        out.push_str(&format!(
            "\n {:>4} | {}\n      | {}^ {}",
            issue.line,
            issue.context.source_line,
            " ".repeat(issue.column.saturating_sub(1)),
            issue.message
        ));
    }
    for related in &issue.context.related {
        out.push_str(&format!("\n{}", related.render()));
    }
    for include in &issue.context.includes {
        out.push_str(&format!("\n  included from {}", include.render()));
    }
    let help = if issue.context.help.is_empty() {
        "fix the labelled value and save the file; the last good configuration remains active"
    } else {
        &issue.context.help
    };
    out.push_str(&format!("\n  help: {}", help));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::decode;
    #[test]
    fn located_source_label_and_help() {
        let d = decode::decode_text("layout {\n gapz 3\n}", "sample.kdl", None);
        let text = render(&d.issues[0], "");
        assert!(text.starts_with("sample.kdl:2:2: error:"), "{text}");
        assert!(text.contains("2 |  gapz 3"));
        assert!(text.contains("^ unknown option"));
        assert!(text.contains("help:"));
    }
}
