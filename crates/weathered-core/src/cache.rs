//! Small on-disk cache with a unix-timestamp sidecar.
//!
//! The payload lives at `path`. `path` + `.time` stores the save time in seconds.

use std::path::{Path, PathBuf};

use crate::error::{IoResultExt, Result};

/// Reuse a cached manifest or loader list for one hour.
pub const FRESH_SECS: i64 = 3600;

/// Seconds since the unix epoch, or zero when the clock is unavailable.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn stamp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".time");
    path.with_file_name(name)
}

/// Read a cache file and its age in seconds. Missing files return `None`.
pub fn read(path: &Path) -> Option<(i64, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let saved = std::fs::read_to_string(stamp_path(path))
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .unwrap_or(0);
    Some((saved, bytes))
}

/// True when `saved_at` is within `max_age` seconds of now.
pub fn is_fresh(saved_at: i64, max_age: i64) -> bool {
    saved_at > 0 && unix_now().saturating_sub(saved_at) < max_age
}

/// Write the payload and a timestamp sidecar.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).io_at(parent)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes).io_at(&tmp)?;
    std::fs::rename(&tmp, path).io_at(path)?;
    let stamp = stamp_path(path);
    std::fs::write(&stamp, format!("{}\n", unix_now())).io_at(stamp)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_write_can_be_read_back() {
        let root = std::env::temp_dir().join(format!(
            "weathered-cache-{}-{}",
            std::process::id(),
            unix_now()
        ));
        let path = root.join("sample.json");
        write(&path, b"{\"ok\":true}").unwrap();
        let (saved, bytes) = read(&path).unwrap();
        assert_eq!(bytes, b"{\"ok\":true}");
        assert!(is_fresh(saved, FRESH_SECS));
        assert!(!is_fresh(saved.saturating_sub(FRESH_SECS + 5), FRESH_SECS));
        let _ = std::fs::remove_dir_all(root);
    }
}
