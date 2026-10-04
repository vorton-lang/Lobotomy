//! Capability trimming through the process environment (harness-adapter.md §1.6). Every harness
//! process gets it: role turns and quota checks alike.

use std::path::Path;

/// gh prefers these to the credentials in its config directory, so an empty config directory
/// alone does not log gh out (#10).
pub const REMOVED_VARS: &[&str] = &["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];

/// Variables to set: `git push` fails locally, and `gh` reads an empty config directory. git also
/// skips the system's attribute file, so line endings in a slot follow the repository only
/// (harness-adapter.md §3).
pub fn env(gh_config_dir: &Path) -> Vec<(String, String)> {
    let mut env = vec![("GIT_CONFIG_COUNT".to_owned(), "3".to_owned())];
    for (i, prefix) in ["https://", "git@", "ssh://"].iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{i}"), "url.lobotomy-push-disabled://.pushInsteadOf".to_owned()));
        env.push((format!("GIT_CONFIG_VALUE_{i}"), (*prefix).to_owned()));
    }
    env.push(("GH_CONFIG_DIR".to_owned(), gh_config_dir.to_string_lossy().into_owned()));
    env.push(("GIT_ATTR_NOSYSTEM".to_owned(), "1".to_owned()));
    env
}
