//! A diff as Angela reads it (data-model.md §10.6): what changed in each file, then the changes as
//! unified text, as far as they fit in one read. The files past that are named, to read by path.

use lobotomy_store::{Content, FileChange};
use serde::Serialize;
use similar::{ChangeTag, TextDiff};

/// How much unified text one read gives.
const TEXT_LIMIT: usize = 60_000;

#[derive(Debug, Serialize)]
pub struct DiffText {
    pub files: Vec<FileLine>,
    /// The changes as unified text, file by file.
    pub diff: String,
    /// Files whose text did not fit in `diff`.
    pub not_shown: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct FileLine {
    pub path: String,
    /// `added`, `deleted` or `modified`.
    pub change: &'static str,
    pub added_lines: usize,
    pub removed_lines: usize,
    /// Why the file has no text in the diff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The changes to `paths`, or to every file when `paths` is empty.
pub fn render(changes: &[FileChange], paths: &[String]) -> DiffText {
    let mut out = DiffText { files: Vec::new(), diff: String::new(), not_shown: Vec::new() };
    for change in changes.iter().filter(|c| paths.is_empty() || paths.contains(&c.path)) {
        let path = change.path.clone();
        let kind = match (&change.old, &change.new) {
            (None, _) => "added",
            (_, None) => "deleted",
            _ => "modified",
        };
        let (Some(old), Some(new)) = (text(&change.old), text(&change.new)) else {
            let note = [&change.new, &change.old].into_iter().flatten().find_map(no_text);
            out.files.push(FileLine { path, change: kind, added_lines: 0, removed_lines: 0, note });
            continue;
        };
        let diff = TextDiff::from_lines(old, new);
        let (mut added_lines, mut removed_lines) = (0, 0);
        for c in diff.iter_all_changes() {
            match c.tag() {
                ChangeTag::Insert => added_lines += 1,
                ChangeTag::Delete => removed_lines += 1,
                ChangeTag::Equal => {}
            }
        }
        let (a, b) = match kind {
            "added" => ("/dev/null".to_owned(), format!("b/{path}")),
            "deleted" => (format!("a/{path}"), "/dev/null".to_owned()),
            _ => (format!("a/{path}"), format!("b/{path}")),
        };
        let unified = diff.unified_diff().context_radius(3).header(&a, &b).to_string();
        if out.diff.len() + unified.len() <= TEXT_LIMIT {
            out.diff.push_str(&unified);
        } else {
            out.not_shown.push(path.clone());
        }
        out.files.push(FileLine { path, change: kind, added_lines, removed_lines, note: None });
    }
    out
}

/// A side's text; a side where the file did not exist is empty.
fn text(side: &Option<Content>) -> Option<&str> {
    match side {
        None => Some(""),
        Some(Content::Text { text }) => Some(text),
        Some(_) => None,
    }
}

fn no_text(side: &Content) -> Option<String> {
    Some(match side {
        Content::Text { .. } => return None,
        Content::Binary { size } => format!("二进制文件，{size} 字节"),
        Content::TooLarge { size } => format!("文件太大（{size} 字节），不显示内容"),
        Content::Symlink { target } => format!("符号链接，指向 {target}"),
        Content::Conflict => "jj 在这里记录了冲突".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Option<Content> {
        Some(Content::Text { text: s.to_owned() })
    }

    #[test]
    fn each_file_says_what_changed_and_the_text_follows() {
        let changes = [
            FileChange { path: "a.txt".into(), old: text("one\ntwo\n"), new: text("one\nthree\nfour\n") },
            FileChange { path: "new.txt".into(), old: None, new: text("x\n") },
            FileChange { path: "logo.png".into(), old: None, new: Some(Content::Binary { size: 9 }) },
        ];
        let out = render(&changes, &[]);
        let lines: Vec<_> =
            out.files.iter().map(|f| (f.path.as_str(), f.change, f.added_lines, f.removed_lines)).collect();
        assert_eq!(lines, [("a.txt", "modified", 2, 1), ("new.txt", "added", 1, 0), ("logo.png", "added", 0, 0)]);
        assert_eq!(out.files[2].note.as_deref(), Some("二进制文件，9 字节"));
        assert!(
            out.diff.contains("--- a/a.txt\n+++ b/a.txt\n") && out.diff.contains("-two\n+three\n+four\n"),
            "{}",
            out.diff
        );
        assert!(out.diff.contains("--- /dev/null\n+++ b/new.txt\n"), "{}", out.diff);
        assert!(out.not_shown.is_empty());

        let only = render(&changes, &["new.txt".into()]);
        assert_eq!(only.files.len(), 1);
        assert!(!only.diff.contains("a.txt"));
    }

    #[test]
    fn what_does_not_fit_is_named_to_read_by_path() {
        let big = "line\n".repeat(TEXT_LIMIT / 5 + 1);
        let changes = [
            FileChange { path: "small.txt".into(), old: None, new: text("x\n") },
            FileChange { path: "big.txt".into(), old: None, new: text(&big) },
        ];
        let out = render(&changes, &[]);
        assert_eq!(out.not_shown, ["big.txt"]);
        assert!(out.diff.contains("small.txt") && !out.diff.contains("big.txt"));
        assert_eq!(out.files[1].added_lines, TEXT_LIMIT / 5 + 1, "the counts cover the whole file");
    }
}
