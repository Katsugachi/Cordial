//! Workspace file tools.
//!
//! Every path is resolved inside the workspace folder. Requests that escape it
//! (absolute paths, `..` traversal, symlinks pointing out) are refused, because
//! an agent that can read `~/.ssh` is not a feature.

use std::path::{Component, Path, PathBuf};

use crate::error::CoreError;
use crate::session::ToolCall;
use crate::tools::{ToolOutcome, ToolRuntime};
use crate::util::{human_bytes, truncate_chars};

const MAX_READ_BYTES: u64 = 1_500_000;
const MAX_WRITE_BYTES: usize = 1_000_000;
const MAX_DIR_ENTRIES: usize = 200;

pub fn read(runtime: &ToolRuntime, call: &ToolCall) -> ToolOutcome {
    let Some(raw) = call.arg("path") else {
        return ToolOutcome::fail("missing path", "Error: `path` is required.");
    };
    let path = match resolve(runtime.workspace(), raw) {
        Ok(path) => path,
        Err(err) => return ToolOutcome::fail("path refused", format!("Error: {err}")),
    };
    if !path.exists() {
        return ToolOutcome::fail(
            "file not found",
            format!(
                "Error: `{}` does not exist. Use `list_files` to see what is there.",
                display_relative(runtime.workspace(), &path)
            ),
        );
    }
    if path.is_dir() {
        return ToolOutcome::fail(
            "path is a folder",
            "Error: that path is a folder. Use `list_files` instead.".to_string(),
        );
    }
    match read_text_bounded(&path, runtime.effort().fetch_chars()) {
        Ok(text) => ToolOutcome::ok(
            format!("read {}", display_relative(runtime.workspace(), &path)),
            text,
        ),
        Err(err) => ToolOutcome::fail("read failed", format!("Error: {err}")),
    }
}

pub fn list(runtime: &ToolRuntime, call: &ToolCall) -> ToolOutcome {
    let raw = call.arg("path").unwrap_or(".");
    let dir = match resolve(runtime.workspace(), raw) {
        Ok(path) => path,
        Err(err) => return ToolOutcome::fail("path refused", format!("Error: {err}")),
    };
    if !dir.is_dir() {
        return ToolOutcome::fail(
            "not a folder",
            format!(
                "Error: `{}` is not a folder in the workspace.",
                display_relative(runtime.workspace(), &dir)
            ),
        );
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) => return ToolOutcome::fail("listing failed", format!("Error: {err}")),
    };

    let mut names: Vec<(String, bool, u64)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = path.is_dir();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        names.push((
            entry.file_name().to_string_lossy().into_owned(),
            is_dir,
            size,
        ));
    }
    names.sort_by(|a, b| (b.1, a.0.to_lowercase()).cmp(&(a.1, b.0.to_lowercase())));

    let mut text = format!(
        "Contents of {} ({} entries):\n",
        display_relative(runtime.workspace(), &dir),
        names.len()
    );
    for (name, is_dir, size) in names.iter().take(MAX_DIR_ENTRIES) {
        if *is_dir {
            text.push_str(&format!("{name}/\n"));
        } else {
            text.push_str(&format!("{name}  {}\n", human_bytes(*size)));
        }
    }
    if names.len() > MAX_DIR_ENTRIES {
        text.push_str("…more entries not listed\n");
    }
    if names.is_empty() {
        text.push_str("(empty folder)\n");
    }
    ToolOutcome::ok(format!("{} entries", names.len()), text)
}

pub fn write(runtime: &ToolRuntime, call: &ToolCall) -> ToolOutcome {
    let (Some(raw), Some(content)) = (call.arg("path"), call.arg("content")) else {
        return ToolOutcome::fail(
            "missing arguments",
            "Error: `path` and `content` are both required.",
        );
    };
    if content.len() > MAX_WRITE_BYTES {
        return ToolOutcome::fail(
            "file too large",
            format!(
                "Error: content is {} but the limit is {}.",
                human_bytes(content.len() as u64),
                human_bytes(MAX_WRITE_BYTES as u64)
            ),
        );
    }
    let path = match resolve(runtime.workspace(), raw) {
        Ok(path) => path,
        Err(err) => return ToolOutcome::fail("path refused", format!("Error: {err}")),
    };
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            return ToolOutcome::fail("write failed", format!("Error: {err}"));
        }
    }
    match std::fs::write(&path, content.as_bytes()) {
        Ok(()) => ToolOutcome::ok(
            format!("wrote {}", display_relative(runtime.workspace(), &path)),
            format!(
                "Wrote {} to {}.",
                human_bytes(content.len() as u64),
                display_relative(runtime.workspace(), &path)
            ),
        ),
        Err(err) => ToolOutcome::fail("write failed", format!("Error: {err}")),
    }
}

/// Reads a UTF-8 text file with a size cap. Returns an error for binaries.
pub fn read_text_bounded(path: &Path, max_chars: usize) -> Result<String, CoreError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_READ_BYTES {
        return Err(CoreError::Other(format!(
            "file is {} which is larger than the {} limit",
            human_bytes(metadata.len()),
            human_bytes(MAX_READ_BYTES)
        )));
    }
    let bytes = std::fs::read(path)?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err(CoreError::Other(
            "this looks like a binary file, not text".to_string(),
        ));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(truncate_chars(&text, max_chars))
}

/// Resolves a user-supplied relative path inside `workspace`.
///
/// The returned path is canonicalised when it already exists. Symlink
/// components are refused rather than followed, so a link inside the workspace
/// cannot be used to read or overwrite a file outside it.
pub(crate) fn resolve(workspace: &Path, raw: &str) -> Result<PathBuf, CoreError> {
    let trimmed = raw.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        return workspace
            .canonicalize()
            .map_err(|err| CoreError::Other(format!("workspace is not readable: {err}")));
    }
    if trimmed.chars().any(|ch| ch.is_control() || ch == '\0') {
        return Err(CoreError::Other(
            "control characters are not allowed in paths".to_string(),
        ));
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return Err(CoreError::Other(
            "absolute paths are not allowed; use a path inside the workspace".to_string(),
        ));
    }
    let root = workspace
        .canonicalize()
        .map_err(|err| CoreError::Other(format!("workspace is not readable: {err}")))?;
    let mut joined = root.clone();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                joined.push(part);
                if let Ok(meta) = std::fs::symlink_metadata(&joined) {
                    if meta.file_type().is_symlink() {
                        return Err(CoreError::Other(
                            "symlinks are not allowed in workspace paths".to_string(),
                        ));
                    }
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if joined == root {
                    return Err(CoreError::Other(
                        "that path is outside the workspace folder".to_string(),
                    ));
                }
                joined.pop();
            }
            _ => {
                return Err(CoreError::Other(
                    "that path is outside the workspace folder".to_string(),
                ));
            }
        }
    }
    if joined.exists() {
        let canonical = joined
            .canonicalize()
            .map_err(|err| CoreError::Other(format!("path is not readable: {err}")))?;
        if !canonical.starts_with(&root) {
            return Err(CoreError::Other(
                "that path is outside the workspace folder".to_string(),
            ));
        }
        return Ok(canonical);
    }
    Ok(joined)
}

fn display_relative(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
        .trim_start_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cordial-files-{tag}-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn relative_paths_resolve_inside_workspace() {
        let workspace = temp_workspace("resolve");
        let path = resolve(&workspace, "notes/today.md").unwrap();
        assert!(path.starts_with(&workspace));
        assert!(path.ends_with("notes/today.md"));
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn traversal_is_refused() {
        let workspace = temp_workspace("traversal");
        assert!(resolve(&workspace, "../../etc/passwd").is_err());
        assert!(resolve(&workspace, "a/../../b").is_err());
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn absolute_paths_are_refused() {
        let workspace = temp_workspace("absolute");
        assert!(resolve(&workspace, "/etc/hosts").is_err());
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn binary_files_are_rejected() {
        let workspace = temp_workspace("binary");
        let file = workspace.join("image.bin");
        std::fs::write(&file, [0u8, 1, 2, 3, 0, 9]).unwrap();
        assert!(read_text_bounded(&file, 1000).is_err());
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_refused() {
        let workspace = temp_workspace("symlink");
        let outside = workspace
            .parent()
            .unwrap()
            .join(format!("outside-{}", crate::util::now_ms()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("link")).unwrap();
        assert!(resolve(&workspace, "link/secret.txt").is_err());
        assert!(resolve(&workspace, "link").is_err());
        std::fs::remove_dir_all(&workspace).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn text_reads_are_truncated() {
        let workspace = temp_workspace("truncate");
        let file = workspace.join("long.txt");
        std::fs::write(&file, "abcdefghij").unwrap();
        let text = read_text_bounded(&file, 4).unwrap();
        assert!(text.starts_with("abcd"));
        assert!(text.contains("cut"));
        std::fs::remove_dir_all(&workspace).ok();
    }
}
