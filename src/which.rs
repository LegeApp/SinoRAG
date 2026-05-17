//! Tiny cross-platform `which` helper. Used by `agent` and `setup` to find
//! external binaries (opencode) without pulling in a `which` crate.
//!
//! On Windows, npm and many other installers ship `.cmd`, `.bat`, or `.ps1`
//! shims rather than `.exe` files. We honor `PATHEXT` (falling back to the
//! `cmd.exe` default list) so those shims are discoverable.

use std::path::{Path, PathBuf};

#[cfg(windows)]
fn pathext_candidates() -> Vec<String> {
    let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    raw.split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

#[cfg(not(windows))]
fn pathext_candidates() -> Vec<String> {
    vec![String::new()]
}

/// Look up `name` on `PATH`, honoring `PATHEXT` on Windows. Returns the
/// first match. `name` may already include an extension, in which case the
/// bare form is tried first.
pub fn locate_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts = pathext_candidates();
    let has_explicit_ext = Path::new(name).extension().is_some();

    for entry in std::env::split_paths(&path) {
        if has_explicit_ext {
            let candidate = entry.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        } else {
            for ext in &exts {
                let candidate = if ext.is_empty() {
                    entry.join(name)
                } else {
                    entry.join(format!("{name}{ext}"))
                };
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Resolve a binary using the standard precedence: explicit flag → env var →
/// PATH lookup. `default_name` is the bare program name (no extension on
/// Windows — `PATHEXT` handles that).
pub fn resolve_binary(
    flag: Option<&Path>,
    env_var: &str,
    default_name: &str,
) -> Option<PathBuf> {
    if let Some(p) = flag {
        return p.is_file().then(|| p.to_path_buf());
    }
    if let Ok(env_path) = std::env::var(env_var) {
        let p = PathBuf::from(env_path);
        if p.is_file() {
            return Some(p);
        }
    }
    locate_on_path(default_name)
}
