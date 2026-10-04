//! Transcript items and the blob store (data-model.md §7).
//!
//! Items are display records, written once when complete. They are not business state, so they
//! are written with `Db::write`, without a command record. A call to a Lobotomy tool keeps only a
//! reference to its command record (data-model.md §7.2).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::id::new_id;

/// Fields larger than this go to the blob store (data-model.md §7.3). The value follows SQLite's
/// own measurement and is revisited with the performance baseline.
pub const STORAGE_THRESHOLD: usize = 100 * 1024;

/// How much of an externalized field stays in the item as a preview, at each end.
const PREVIEW_BYTES: usize = 2 * 1024;

/// Content-addressed files: named by their SHA-256, never modified, stored once per content.
pub struct BlobStore {
    dir: PathBuf,
}

/// A file written to the blob store, waiting for its row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Blob {
    pub hash: String,
    pub size: i64,
    pub head: String,
    pub tail: String,
}

impl BlobStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path(&self, hash: &str) -> PathBuf {
        self.dir.join(&hash[..2]).join(hash)
    }

    /// Writes the file unless it already exists. Called before the transaction that inserts the
    /// blob row: a crash in between leaves a file without a row, which the startup sweep deletes
    /// (data-model.md §6). The reverse order could leave a row without its file.
    pub fn put(&self, text: &str) -> Result<Blob> {
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        let path = self.path(&hash);
        if !path.exists() {
            let dir = path.parent().expect("blob paths have a parent");
            fs::create_dir_all(dir)?;
            let tmp = dir.join(format!("{hash}.{}.tmp", new_id("tmp")));
            let mut file = fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            if let Err(e) = fs::rename(&tmp, &path) {
                let _ = fs::remove_file(&tmp);
                if !path.exists() {
                    return Err(e.into());
                }
            }
        }
        Ok(Blob {
            hash,
            size: text.len() as i64,
            head: prefix(text, PREVIEW_BYTES).to_owned(),
            tail: suffix(text, PREVIEW_BYTES).to_owned(),
        })
    }

    pub fn read(&self, hash: &str) -> Result<String> {
        Ok(fs::read_to_string(self.path(hash))?)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn prefix(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn suffix(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Moves the large string fields of an item's content into the blob store. Each such field
/// becomes `{"blob": hash, "size": n, "head": …, "tail": …}`. Runs before the transaction; pass
/// the returned blobs to [`record_item`].
///
/// A field whose file cannot be written stays in the content, so the item is still stored, in
/// order, with its full text; only reading that field is slower (#10).
pub fn externalize(store: &BlobStore, content: &mut Value) -> Vec<Blob> {
    let mut blobs = Vec::new();
    if let Value::Object(fields) = content {
        for (name, value) in fields.iter_mut() {
            if let Value::String(text) = value
                && text.len() > STORAGE_THRESHOLD
            {
                match store.put(text) {
                    Ok(blob) => {
                        *value = json!({ "blob": blob.hash, "size": blob.size, "head": blob.head, "tail": blob.tail });
                        blobs.push(blob);
                    }
                    Err(e) => {
                        tracing::warn!(field = name, error = %e, "blob not written; the field stays in the database")
                    }
                }
            }
        }
    }
    blobs
}

/// Inserts the rows of blobs [`externalize`] wrote, in the transaction that references them.
pub fn record_blobs(tx: &Transaction<'_>, blobs: &[Blob], now: i64) -> Result<()> {
    for blob in blobs {
        tx.execute(
            "INSERT OR IGNORE INTO blob (hash, size, head, tail, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![blob.hash, blob.size, blob.head, blob.tail, now],
        )?;
    }
    Ok(())
}

/// One transcript item to store.
#[derive(Clone, Debug)]
pub struct NewItem<'a> {
    pub role: &'a str,
    pub turn_id: &'a str,
    /// The harness's id for the item, unique within the turn. A second item with the same id is
    /// ignored, so replaying the CLI output is harmless.
    pub native_item_id: Option<&'a str>,
    pub kind: &'a str,
    pub content: &'a Value,
    pub command_id: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StoredItem {
    pub id: String,
    pub thread_id: String,
    pub seq: i64,
}

/// The role's thread, created on first use. Threads are per role and span native sessions
/// (data-model.md §7.1).
pub fn thread_of(tx: &Transaction<'_>, role: &str) -> Result<String> {
    if let Some(id) = tx.query_row("SELECT id FROM thread WHERE role = ?1", [role], |r| r.get(0)).optional()? {
        return Ok(id);
    }
    let id = new_id("thr");
    tx.execute("INSERT INTO thread (id, role) VALUES (?1, ?2)", params![id, role])?;
    Ok(id)
}

/// Stores a completed item with the next sequence number of its thread. Returns `None` when an
/// item with the same native id was already stored for the turn.
pub fn record_item(tx: &Transaction<'_>, item: &NewItem<'_>, blobs: &[Blob], now: i64) -> Result<Option<StoredItem>> {
    if let Some(native) = item.native_item_id {
        let exists = tx
            .query_row(
                "SELECT 1 FROM item WHERE turn_id = ?1 AND native_item_id = ?2",
                params![item.turn_id, native],
                |_| Ok(()),
            )
            .optional()?;
        if exists.is_some() {
            return Ok(None);
        }
    }
    record_blobs(tx, blobs, now)?;
    let thread_id = thread_of(tx, item.role)?;
    let seq: i64 =
        tx.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM item WHERE thread_id = ?1", [&thread_id], |r| r.get(0))?;
    let id = new_id("item");
    tx.execute(
        "INSERT INTO item (id, thread_id, seq, turn_id, native_item_id, kind, content, command_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            thread_id,
            seq,
            item.turn_id,
            item.native_item_id,
            item.kind,
            item.content.to_string(),
            item.command_id,
            now
        ],
    )?;
    Ok(Some(StoredItem { id, thread_id, seq }))
}
