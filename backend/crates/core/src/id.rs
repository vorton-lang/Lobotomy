use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// One generator for the process. Ids made in the same millisecond still sort in the order they
/// were made: queries break ties of `created_at` by id. Plain ULIDs are random within a
/// millisecond, which made "the latest capture" a coin toss in tests.
static IDS: Mutex<ulid::Generator> = Mutex::new(ulid::Generator::new());

/// A globally unique id with a type prefix, such as `task_01J…` (data-model.md §7.7).
pub fn new_id(prefix: &str) -> String {
    let mut ids = IDS.lock().unwrap_or_else(|e| e.into_inner());
    let id = ids.generate().unwrap_or_else(|overflow| overflow.commit_overflow_increment());
    format!("{prefix}_{id}")
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_sort_in_the_order_they_were_made() {
        let ids: Vec<String> = (0..1000).map(|_| new_id("cap")).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }
}
