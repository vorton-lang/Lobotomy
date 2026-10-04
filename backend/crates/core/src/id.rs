use std::time::{SystemTime, UNIX_EPOCH};

/// A globally unique id with a type prefix, such as `task_01J…` (data-model.md §7.7).
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::generate())
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
