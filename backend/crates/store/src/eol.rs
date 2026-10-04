//! Line endings follow the repository: its `.gitattributes` and each file's existing style
//! (harness-adapter.md §3, §4.1). Nothing on the machine applies: not `core.autocrlf`, not
//! `core.eol`, not the operating system.
//!
//! The rules, per file:
//! - Declared text (`text`, `text=auto` for text content, or an `eol` attribute): stored with LF,
//!   as git normalizes it; written with CRLF where `eol=crlf` says so.
//! - Declared not text (`-text`, `binary`): stored and written byte for byte.
//! - Not declared: a changed file keeps the line endings it has at the baseline; a new text file
//!   gets LF. Git itself would store whatever the agent's tools wrote.
//!
//! Text detection and the `text=auto` details follow git's convert.c.

use std::collections::HashMap;
use std::path::Path;

use futures::StreamExt as _;
use futures::io::AsyncReadExt as _;
use jj_lib::backend::TreeValue;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::merge::Merge;
use jj_lib::merged_tree::MergedTree;
use jj_lib::merged_tree_builder::MergedTreeBuilder;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use pollster::FutureExt as _;

use crate::error::{Error, Result};
use crate::git::Git;
use crate::store::Store;

/// Character counts as git's `gather_stats` makes them.
#[derive(Default)]
struct Stats {
    crlf: usize,
    lone_cr: usize,
    lone_lf: usize,
    nul: usize,
    printable: usize,
    nonprintable: usize,
}

fn stats(buf: &[u8]) -> Stats {
    let mut s = Stats::default();
    let mut i = 0;
    while i < buf.len() {
        let c = buf[i];
        match c {
            b'\r' if buf.get(i + 1) == Some(&b'\n') => {
                s.crlf += 1;
                i += 1;
            }
            b'\r' => s.lone_cr += 1,
            b'\n' => s.lone_lf += 1,
            127 => s.nonprintable += 1,
            b'\x08' | b'\t' | b'\x1b' | b'\x0c' => s.printable += 1,
            0 => {
                s.nul += 1;
                s.nonprintable += 1;
            }
            c if c < 32 => s.nonprintable += 1,
            _ => s.printable += 1,
        }
        i += 1;
    }
    // A trailing ^Z (DOS end of file) does not count as non-printable.
    if buf.last() == Some(&0x1a) {
        s.nonprintable -= 1;
    }
    s
}

impl Stats {
    /// git's `convert_is_binary`.
    fn is_binary(&self) -> bool {
        self.lone_cr > 0 || self.nul > 0 || (self.printable >> 7) < self.nonprintable
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Text {
    Set,
    Unset,
    Auto,
    Unspecified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Eol {
    Lf,
    Crlf,
    Unspecified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Attrs {
    text: Text,
    eol: Eol,
}

/// The `text` and `eol` attributes of `paths`, as git resolves them in `dir` from the
/// repository's `.gitattributes`. Attribute files outside the repository are not read: the slot
/// points `core.attributesFile` nowhere, and the system file is switched off here.
fn attributes(dir: &Path, paths: &[RepoPathBuf]) -> Result<HashMap<RepoPathBuf, Attrs>> {
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(path.as_internal_file_string().as_bytes());
        input.push(0);
    }
    let out = Git::at(dir).run_with_input(
        &["check-attr", "-z", "--stdin", "text", "eol"],
        &input,
        &[("GIT_ATTR_NOSYSTEM", "1")],
    )?;
    let mut attrs: HashMap<RepoPathBuf, Attrs> = HashMap::new();
    let fields: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    for record in fields.as_chunks::<3>().0 {
        let path = RepoPathBuf::from_internal_string(String::from_utf8_lossy(record[0]).into_owned()).map_err(Error::jj)?;
        let entry = attrs.entry(path).or_insert(Attrs { text: Text::Unspecified, eol: Eol::Unspecified });
        match (record[1], record[2]) {
            (b"text", b"set") => entry.text = Text::Set,
            (b"text", b"unset") => entry.text = Text::Unset,
            (b"text", b"auto") => entry.text = Text::Auto,
            (b"eol", b"lf") => entry.eol = Eol::Lf,
            (b"eol", b"crlf") => entry.eol = Eol::Crlf,
            _ => {}
        }
    }
    Ok(attrs)
}

fn to_lf(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == b'\r' && buf.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(buf[i]);
        i += 1;
    }
    out
}

fn to_crlf(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len() + buf.len() / 32);
    for (i, &b) in buf.iter().enumerate() {
        if b == b'\n' && (i == 0 || buf[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

/// What is stored for `content`: `None` when it is stored as it is.
fn repository_form(content: &[u8], attrs: Attrs, baseline: Option<&[u8]>) -> Option<Vec<u8>> {
    let s = stats(content);
    let declared_text = match (attrs.text, attrs.eol) {
        (Text::Unset, _) => return None,
        (Text::Set, _) | (Text::Unspecified, Eol::Lf | Eol::Crlf) => true,
        // git leaves a file alone that is binary, or that the repository already holds with CRLF.
        (Text::Auto, _) => {
            if s.is_binary() || baseline.is_some_and(|b| stats(b).crlf > 0) {
                return None;
            }
            true
        }
        (Text::Unspecified, Eol::Unspecified) => false,
    };
    if declared_text {
        return (s.crlf > 0).then(|| to_lf(content));
    }
    if s.is_binary() {
        return None;
    }
    let crlf = match baseline.map(stats) {
        Some(b) if b.is_binary() => return None,
        Some(b) if b.crlf > 0 && b.lone_lf > 0 => return None, // mixed: no style to keep
        Some(b) if b.crlf > 0 => true,
        _ => false, // LF, no line break yet, or a new file
    };
    match crlf {
        true if s.lone_lf > 0 => Some(to_crlf(content)),
        false if s.crlf > 0 => Some(to_lf(content)),
        _ => None,
    }
}

/// What is written to disk for stored content: CRLF where the repository says `eol=crlf`.
fn worktree_form(stored: &[u8], attrs: Attrs) -> Option<Vec<u8>> {
    if attrs.text == Text::Unset || attrs.eol != Eol::Crlf {
        return None;
    }
    let s = stats(stored);
    if attrs.text == Text::Auto && s.is_binary() {
        return None;
    }
    (s.lone_lf > 0 && s.crlf == 0).then(|| to_crlf(stored))
}

async fn read(store: &Store, path: &RepoPath, value: &TreeValue) -> Result<Vec<u8>> {
    let TreeValue::File { id, .. } = value else { unreachable!("only files are read") };
    let mut reader = store.inner.read_file(path, id).await.map_err(Error::jj)?;
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.map_err(Error::io(path.as_internal_file_string()))?;
    Ok(buf)
}

/// Regular files whose content differs between the two trees, with their value in `to`.
async fn changed_files(from: &MergedTree, to: &MergedTree) -> Result<Vec<(RepoPathBuf, TreeValue)>> {
    let mut changed = Vec::new();
    let mut diff = from.diff_stream(to, &EverythingMatcher);
    while let Some(entry) = diff.next().await {
        let values = entry.values.map_err(Error::jj)?;
        if let Some(Some(value @ TreeValue::File { .. })) = values.after.as_resolved() {
            changed.push((entry.path, value.clone()));
        }
    }
    Ok(changed)
}

/// Brings the files a capture changed (from `before` to `after`) to the repository's line
/// endings. Returns the tree to commit when anything was rewritten, and writes the files back to
/// `dir` in their working-tree form, so the agent and git in the slot see what was captured.
pub(crate) fn normalize_capture(
    store: &Store,
    dir: &Path,
    before: &MergedTree,
    after: &MergedTree,
    baseline: &MergedTree,
) -> Result<Option<MergedTree>> {
    async {
        let changed = changed_files(before, after).await?;
        if changed.is_empty() {
            return Ok(None);
        }
        let paths: Vec<RepoPathBuf> = changed.iter().map(|(p, _)| p.clone()).collect();
        let attrs = attributes(dir, &paths)?;
        let mut builder = MergedTreeBuilder::new(after.clone());
        let mut rewritten = false;
        for (path, value) in changed {
            let attrs = attrs[&path];
            let content = read(store, &path, &value).await?;
            let base = match baseline.path_value(&path).await.map_err(Error::jj)?.as_resolved() {
                Some(Some(base @ TreeValue::File { .. })) => Some(read(store, &path, base).await?),
                _ => None,
            };
            let stored = match repository_form(&content, attrs, base.as_deref()) {
                Some(stored) => {
                    let id = store.inner.write_file(&path, &mut stored.as_slice()).await.map_err(Error::jj)?;
                    let TreeValue::File { executable, copy_id, .. } = value else { unreachable!() };
                    builder.set_or_remove(path.clone(), Merge::normal(TreeValue::File { id, executable, copy_id }));
                    rewritten = true;
                    stored
                }
                None => content.clone(),
            };
            let on_disk = worktree_form(&stored, attrs).unwrap_or(stored);
            if on_disk != content {
                let disk = path.to_fs_path(dir).map_err(Error::jj)?;
                std::fs::write(&disk, &on_disk).map_err(Error::io(&disk))?;
            }
        }
        if !rewritten {
            return Ok(None);
        }
        Ok(Some(builder.write_tree().await.map_err(Error::jj)?))
    }
    .block_on()
}

/// After a checkout from `before` to `after`, writes CRLF into the checked-out files whose
/// `eol` attribute asks for it, as git would check them out.
pub(crate) fn apply_to_worktree(store: &Store, dir: &Path, before: &MergedTree, after: &MergedTree) -> Result<()> {
    async {
        let written = changed_files(before, after).await?;
        if written.is_empty() {
            return Ok(());
        }
        let paths: Vec<RepoPathBuf> = written.iter().map(|(p, _)| p.clone()).collect();
        let attrs = attributes(dir, &paths)?;
        for (path, value) in written {
            let attrs = attrs[&path];
            if attrs.eol != Eol::Crlf {
                continue;
            }
            let stored = read(store, &path, &value).await?;
            if let Some(on_disk) = worktree_form(&stored, attrs) {
                let disk = path.to_fs_path(dir).map_err(Error::jj)?;
                std::fs::write(&disk, &on_disk).map_err(Error::io(&disk))?;
            }
        }
        Ok(())
    }
    .block_on()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Attrs = Attrs { text: Text::Unspecified, eol: Eol::Unspecified };
    const AUTO: Attrs = Attrs { text: Text::Auto, eol: Eol::Unspecified };

    #[test]
    fn undeclared_files_keep_their_baseline_style() {
        // LF file rewritten with CRLF by a Windows tool.
        assert_eq!(repository_form(b"a\r\nb\r\n", NONE, Some(b"a\n")).as_deref(), Some(&b"a\nb\n"[..]));
        // CRLF file patched with LF lines.
        assert_eq!(repository_form(b"a\r\nb\nc\r\n", NONE, Some(b"a\r\n")).as_deref(), Some(&b"a\r\nb\r\nc\r\n"[..]));
        // A new file gets LF.
        assert_eq!(repository_form(b"a\r\n", NONE, None).as_deref(), Some(&b"a\n"[..]));
        // Already in style, or a mixed baseline: stored as written.
        assert_eq!(repository_form(b"a\nb\n", NONE, Some(b"a\n")), None);
        assert_eq!(repository_form(b"a\r\nb\n", NONE, Some(b"x\r\ny\n")), None);
        // Binary content is never touched.
        assert_eq!(repository_form(b"a\0\r\n", NONE, Some(b"a\n")), None);
    }

    #[test]
    fn declared_text_is_stored_with_lf() {
        let set = Attrs { text: Text::Set, eol: Eol::Unspecified };
        assert_eq!(repository_form(b"a\r\n", set, Some(b"a\r\n")).as_deref(), Some(&b"a\n"[..]));
        let crlf = Attrs { text: Text::Unspecified, eol: Eol::Crlf };
        assert_eq!(repository_form(b"a\r\n", crlf, None).as_deref(), Some(&b"a\n"[..]));
        assert_eq!(repository_form(b"a\r\n", AUTO, Some(b"a\n")).as_deref(), Some(&b"a\n"[..]));
        // git's text=auto leaves binary content and files the repository holds with CRLF.
        assert_eq!(repository_form(b"a\0\r\n", AUTO, None), None);
        assert_eq!(repository_form(b"a\r\nb\r\n", AUTO, Some(b"a\r\n")), None);
        let unset = Attrs { text: Text::Unset, eol: Eol::Unspecified };
        assert_eq!(repository_form(b"a\r\n", unset, Some(b"a\n")), None);
    }

    #[test]
    fn eol_crlf_is_written_with_crlf() {
        let crlf = Attrs { text: Text::Auto, eol: Eol::Crlf };
        assert_eq!(worktree_form(b"a\nb\n", crlf).as_deref(), Some(&b"a\r\nb\r\n"[..]));
        assert_eq!(worktree_form(b"a\nb\n", AUTO), None);
        assert_eq!(worktree_form(b"a\0\n", crlf), None);
    }
}
