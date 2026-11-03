//! Confined, read-only command runner.
//!
//! This is deliberately not a shell. The model may ask for a small set of
//! familiar read-only commands (`grep`, `find`, `ls`, `cat`, `head`, `tail`,
//! `wc`, `stat`, `pwd`), but Cordial implements them itself inside the workspace.
//! There is no PATH lookup, no subprocess, no shell interpolation, no network,
//! and no write path. Destructive commands are therefore impossible by
//! construction, not merely matched by a blocklist.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use regex::RegexBuilder;

use crate::session::ToolCall;
use crate::tools::{files, ToolOutcome, ToolRuntime};
use crate::util::truncate_chars;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT_CHARS: usize = 120_000;
const MAX_FILE_BYTES: u64 = 1_500_000;
const MAX_ENTRIES: usize = 5_000;
const MAX_DEPTH: usize = 20;
const MAX_MATCHES: usize = 2_000;

const ALLOWED: &[&str] = &["pwd", "ls", "cat", "head", "tail", "wc", "grep", "find", "stat"];

/// Commands that must never be accepted, even as arguments to another command.
/// The allowlist above is the security boundary; this list only makes denial
/// messages and regression tests explicit.
const DESTRUCTIVE: &[&str] = &[
    "rm", "rmdir", "del", "erase", "rd", "remove-item", "mv", "move", "move-item", "cp", "copy",
    "copy-item", "rename", "rename-item", "chmod", "chown", "chgrp", "attrib", "icacls", "takeown",
    "dd", "mkfs", "fdisk", "diskpart", "format", "shred", "truncate", "tee", "sed", "awk", "perl",
    "python", "python3", "node", "ruby", "php", "lua", "sh", "bash", "zsh", "fish", "dash", "ksh",
    "csh", "tcsh", "cmd", "powershell", "pwsh", "sudo", "su", "doas", "runas", "ssh", "scp",
    "sftp", "rsync", "curl", "wget", "nc", "netcat", "ncat", "telnet", "ftp", "git", "hg", "svn",
    "npm", "npx", "pnpm", "yarn", "pip", "pip3", "cargo", "rustc", "gcc", "g++", "clang", "make",
    "cmake", "ninja", "docker", "podman", "kubectl", "systemctl", "service", "launchctl", "schtasks",
    "crontab", "at", "batch", "kill", "pkill", "killall", "taskkill", "shutdown", "reboot", "halt",
    "poweroff", "mount", "umount", "ln", "link", "touch", "mkdir", "md", "new-item", "set-content",
    "add-content", "out-file", "export-csv", "reg", "regedit", "wmic", "msiexec", "rundll32",
    "regsvr32", "certutil", "bitsadmin", "vssadmin", "bcdedit", "cipher", "fsutil", "iptables",
    "nft", "ufw", "setfacl", "chflags", "xattr", "open", "xdg-open", "start", "invoke-item",
    "invoke-expression", "iex", "eval", "exec", "xargs", "findstr", "where", "which", "env", "printenv",
    "set", "export", "source", ".",
];

pub async fn run(runtime: &ToolRuntime, call: &ToolCall) -> ToolOutcome {
    let Some(command) = call.arg("command") else {
        return ToolOutcome::fail("missing command", "Error: `command` is required.");
    };
    let workspace = std::fs::canonicalize(runtime.workspace())
        .unwrap_or_else(|_| runtime.workspace().to_path_buf());
    let command = command.to_string();
    let result = tokio::time::timeout(TIMEOUT, async {
        tokio::task::spawn_blocking(move || execute(&workspace, &command))
            .await
            .unwrap_or_else(|err| ToolOutcome::fail("command failed", format!("Error: {err}")))
    })
    .await;
    match result {
        Ok(outcome) => outcome,
        Err(_) => ToolOutcome::fail(
            "command timed out",
            "Error: the read-only command exceeded the 10 second limit.",
        ),
    }
}

fn execute(workspace: &Path, command: &str) -> ToolOutcome {
    if command.chars().count() > 4096 {
        return ToolOutcome::fail("command too long", "Error: command exceeds 4096 characters.");
    }
    if has_unquoted_shell_syntax(command) {
        return ToolOutcome::fail(
            "shell syntax refused",
            "Error: pipes, redirects, command substitution and command separators are not allowed. Use one read-only command.",
        );
    }
    let tokens = match shlex::split(command) {
        Some(tokens) if !tokens.is_empty() => tokens,
        _ => return ToolOutcome::fail("empty command", "Error: no command was provided."),
    };
    let name = tokens[0].to_ascii_lowercase();
    if DESTRUCTIVE.contains(&name.as_str()) || !ALLOWED.contains(&name.as_str()) {
        return ToolOutcome::fail(
            "command not allowed",
            format!(
                "Error: `{}` is not allowed. Only these read-only commands are available: {}.",
                tokens[0],
                ALLOWED.join(", ")
            ),
        );
    }
    let args = &tokens[1..];
    if args.iter().any(|arg| arg.chars().any(|ch| ch.is_control())) {
        return ToolOutcome::fail("argument refused", "Error: control characters are not allowed.");
    }
    let started = Instant::now();
    let result = match name.as_str() {
        "pwd" => Ok(workspace.display().to_string()),
        "ls" => ls(workspace, args, started),
        "cat" => cat(workspace, args, false, started),
        "head" => head_tail(workspace, args, true, started),
        "tail" => head_tail(workspace, args, false, started),
        "wc" => wc(workspace, args, started),
        "grep" => grep(workspace, args, started),
        "find" => find(workspace, args, started),
        "stat" => stat(workspace, args, started),
        _ => unreachable!("allowlist checked"),
    };
    match result {
        Ok(text) => {
            let text = truncate_chars(&text, MAX_OUTPUT_CHARS);
            ToolOutcome::ok(format!("ran {name}"), text)
        }
        Err(err) => ToolOutcome::fail("command failed", format!("Error: {err}")),
    }
}

/// Rejects shell control operators outside quotes. Quoted grep patterns may
/// contain regex metacharacters, but they are never interpreted by a shell.
fn has_unquoted_shell_syntax(command: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ';' | '&' | '|' | '<' | '>' | '`' | '\n' | '\r' => return true,
            '$' if matches!(chars.peek(), Some('(') | Some('{')) => return true,
            '(' | ')' => return true,
            _ => {}
        }
    }
    quote.is_some()
}

fn resolve(workspace: &Path, raw: &str) -> Result<PathBuf, String> {
    files::resolve(workspace, raw).map_err(|err| err.to_string())
}

fn relative(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn check_time(started: Instant) -> Result<(), String> {
    if started.elapsed() > TIMEOUT {
        Err("command exceeded the time limit".to_string())
    } else {
        Ok(())
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|err| err.to_string())?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("{} is larger than the 1.5 MB read limit", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Err(format!("{} looks binary", path.display()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn ls(workspace: &Path, args: &[String], started: Instant) -> Result<String, String> {
    let mut all = false;
    let mut long = false;
    let mut path = ".";
    for arg in args {
        match arg.as_str() {
            "-a" | "--all" => all = true,
            "-l" => long = true,
            "-la" | "-al" => {
                all = true;
                long = true;
            }
            other if other.starts_with('-') => return Err(format!("ls option not allowed: {other}")),
            other => path = other,
        }
    }
    let dir = resolve(workspace, path)?;
    let entries = std::fs::read_dir(&dir).map_err(|err| err.to_string())?;
    let mut rows = Vec::new();
    for (index, entry) in entries.enumerate() {
        check_time(started)?;
        if index >= MAX_ENTRIES {
            rows.push("…entry limit reached".to_string());
            break;
        }
        let entry = entry.map_err(|err| err.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !all && name.starts_with('.') {
            continue;
        }
        let meta = entry.metadata().map_err(|err| err.to_string())?;
        if long {
            rows.push(format!(
                "{} {:>10} {}",
                if meta.is_dir() { "d" } else { "-" },
                meta.len(),
                name
            ));
        } else {
            rows.push(if meta.is_dir() { format!("{name}/") } else { name });
        }
    }
    rows.sort();
    Ok(rows.join("\n"))
}

fn cat(workspace: &Path, args: &[String], number: bool, started: Instant) -> Result<String, String> {
    let mut files_arg = Vec::new();
    let mut number = number;
    for arg in args {
        match arg.as_str() {
            "-n" | "--number" => number = true,
            other if other.starts_with('-') => return Err(format!("cat option not allowed: {other}")),
            other => files_arg.push(other),
        }
    }
    if files_arg.is_empty() {
        return Err("cat requires at least one file".to_string());
    }
    let mut out = String::new();
    for raw in files_arg {
        check_time(started)?;
        let path = resolve(workspace, raw)?;
        let text = read_text(&path)?;
        if number {
            for (index, line) in text.lines().enumerate() {
                out.push_str(&format!("{:>6}  {line}\n", index + 1));
            }
        } else {
            out.push_str(&text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
        if out.chars().count() > MAX_OUTPUT_CHARS {
            out = truncate_chars(&out, MAX_OUTPUT_CHARS);
            break;
        }
    }
    Ok(out)
}

fn head_tail(workspace: &Path, args: &[String], head: bool, started: Instant) -> Result<String, String> {
    let mut count = 10usize;
    let mut path = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-n" | "--lines" => {
                let value = iter.next().ok_or("missing line count")?;
                count = value.parse::<usize>().map_err(|_| "invalid line count")?.min(10_000);
            }
            other if other.starts_with("-n") => {
                count = other[2..].parse::<usize>().map_err(|_| "invalid line count")?.min(10_000);
            }
            other if other.starts_with('-') => return Err(format!("option not allowed: {other}")),
            other => path = Some(other),
        }
    }
    let path = resolve(workspace, path.ok_or("missing file path")?)?;
    check_time(started)?;
    let text = read_text(&path)?;
    let lines: Vec<&str> = text.lines().collect();
    let selected = if head {
        lines.iter().take(count).copied().collect::<Vec<_>>()
    } else {
        lines.iter().rev().take(count).rev().copied().collect::<Vec<_>>()
    };
    Ok(selected.join("\n"))
}

fn wc(workspace: &Path, args: &[String], started: Instant) -> Result<String, String> {
    let mut lines = false;
    let mut words = false;
    let mut chars = false;
    let mut paths = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-l" => lines = true,
            "-w" => words = true,
            "-c" | "-m" => chars = true,
            other if other.starts_with('-') => return Err(format!("wc option not allowed: {other}")),
            other => paths.push(other),
        }
    }
    if !lines && !words && !chars {
        lines = true;
        words = true;
        chars = true;
    }
    if paths.is_empty() {
        return Err("wc requires at least one file".to_string());
    }
    let mut out = String::new();
    for raw in paths {
        check_time(started)?;
        let path = resolve(workspace, raw)?;
        let text = read_text(&path)?;
        let mut parts = Vec::new();
        if lines {
            parts.push(text.lines().count().to_string());
        }
        if words {
            parts.push(text.split_whitespace().count().to_string());
        }
        if chars {
            parts.push(text.chars().count().to_string());
        }
        out.push_str(&format!("{} {}\n", parts.join(" "), relative(workspace, &path)));
    }
    Ok(out)
}

fn grep(workspace: &Path, args: &[String], started: Instant) -> Result<String, String> {
    let mut ignore_case = false;
    let mut line_numbers = false;
    let mut recursive = false;
    let mut files_only = false;
    let mut fixed = false;
    let mut invert = false;
    let mut pattern = None;
    let mut paths = Vec::new();
    let mut options_done = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if options_done {
            if pattern.is_none() {
                pattern = Some(arg.to_string());
            } else {
                paths.push(arg.to_string());
            }
            continue;
        }
        match arg.as_str() {
            "--" => options_done = true,
            "-i" | "--ignore-case" => ignore_case = true,
            "-n" | "--line-number" => line_numbers = true,
            "-r" | "-R" | "--recursive" => recursive = true,
            "-l" | "--files-with-matches" => files_only = true,
            "-F" | "--fixed-strings" => fixed = true,
            "-E" | "--extended-regexp" => {}
            "-v" | "--invert-match" => invert = true,
            "-e" => pattern = Some(iter.next().ok_or("missing pattern")?.clone()),
            other if other.starts_with('-') && other.len() > 2 && !other.starts_with("--") && other[1..].chars().all(|ch| matches!(ch, 'i' | 'n' | 'r' | 'R' | 'l' | 'F' | 'E' | 'v')) => {
                for ch in other[1..].chars() {
                    match ch {
                        'i' => ignore_case = true,
                        'n' => line_numbers = true,
                        'r' | 'R' => recursive = true,
                        'l' => files_only = true,
                        'F' => fixed = true,
                        'E' => {}
                        'v' => invert = true,
                        _ => unreachable!("checked above"),
                    }
                }
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("grep option not allowed: {other}"));
            }
            other => {
                if pattern.is_none() {
                    pattern = Some(other.to_string());
                } else {
                    paths.push(other.to_string());
                }
            }
        }
    }
    let pattern = pattern.ok_or("grep requires a pattern")?;
    if pattern.len() > 4096 {
        return Err("grep pattern is too long".to_string());
    }
    let escaped;
    let pattern_ref = if fixed {
        escaped = regex::escape(&pattern);
        &escaped
    } else {
        &pattern
    };
    let regex = RegexBuilder::new(pattern_ref)
        .case_insensitive(ignore_case)
        .size_limit(1 << 20)
        .build()
        .map_err(|err| format!("invalid grep pattern: {err}"))?;
    if paths.is_empty() {
        paths.push(".".to_string());
    }
    let mut out = String::new();
    let mut matches = 0usize;
    for raw in paths {
        check_time(started)?;
        let path = resolve(workspace, &raw)?;
        let meta = std::fs::symlink_metadata(&path).map_err(|err| err.to_string())?;
        if meta.is_dir() {
            if !recursive {
                return Err(format!("{} is a directory; use -r", relative(workspace, &path)));
            }
            grep_dir(workspace, &path, &regex, line_numbers, files_only, invert, started, &mut out, &mut matches)?;
        } else if meta.is_file() {
            grep_file(workspace, &path, &regex, line_numbers, files_only, invert, started, &mut out, &mut matches)?;
        }
        if matches >= MAX_MATCHES {
            out.push_str("…match limit reached\n");
            break;
        }
    }
    if out.is_empty() {
        Ok("No matches.".to_string())
    } else {
        Ok(out)
    }
}

fn grep_dir(
    workspace: &Path,
    dir: &Path,
    regex: &regex::Regex,
    line_numbers: bool,
    files_only: bool,
    invert: bool,
    started: Instant,
    out: &mut String,
    matches: &mut usize,
) -> Result<(), String> {
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        check_time(started)?;
        let entries = std::fs::read_dir(&dir).map_err(|err| err.to_string())?;
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            check_time(started)?;
            visited += 1;
            if visited > MAX_ENTRIES || *matches >= MAX_MATCHES {
                return Ok(());
            }
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|err| err.to_string())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if meta.is_file() {
                grep_file(workspace, &path, regex, line_numbers, files_only, invert, started, out, matches)?;
            }
        }
    }
    Ok(())
}

fn grep_file(
    workspace: &Path,
    path: &Path,
    regex: &regex::Regex,
    line_numbers: bool,
    files_only: bool,
    invert: bool,
    started: Instant,
    out: &mut String,
    matches: &mut usize,
) -> Result<(), String> {
    check_time(started)?;
    let Ok(text) = read_text(path) else {
        return Ok(());
    };
    let mut matched = false;
    for (index, line) in text.lines().enumerate() {
        if regex.is_match(line) != invert {
            matched = true;
            *matches += 1;
            if files_only {
                out.push_str(&format!("{}\n", relative(workspace, path)));
                break;
            }
            if line_numbers {
                out.push_str(&format!("{}:{}:{line}\n", relative(workspace, path), index + 1));
            } else {
                out.push_str(&format!("{}: {line}\n", relative(workspace, path)));
            }
            if *matches >= MAX_MATCHES || out.chars().count() > MAX_OUTPUT_CHARS {
                return Ok(());
            }
        }
    }
    let _ = matched;
    Ok(())
}

fn find(workspace: &Path, args: &[String], started: Instant) -> Result<String, String> {
    let mut path = ".".to_string();
    let mut name = None;
    let mut kind = None;
    let mut max_depth = MAX_DEPTH;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-name" | "-iname" => name = Some((iter.next().ok_or("missing -name pattern")?.clone(), arg == "-iname")),
            "-type" => kind = Some(iter.next().ok_or("missing -type")?.clone()),
            "-maxdepth" => max_depth = iter.next().ok_or("missing -maxdepth")?.parse::<usize>().map_err(|_| "invalid -maxdepth")?.min(MAX_DEPTH),
            other if other.starts_with('-') => return Err(format!("find option not allowed: {other}; -exec and -delete are never available")),
            other => path = other.to_string(),
        }
    }
    if let Some(kind) = &kind {
        if kind != "f" && kind != "d" {
            return Err("find -type must be f or d".to_string());
        }
    }
    let root = resolve(workspace, &path)?;
    let mut out = Vec::new();
    let mut stack = vec![(root, 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        check_time(started)?;
        let entries = std::fs::read_dir(&dir).map_err(|err| err.to_string())?;
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            check_time(started)?;
            visited += 1;
            if visited > MAX_ENTRIES {
                out.push("…entry limit reached".to_string());
                break;
            }
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|err| err.to_string())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            let name_matches = match &name {
                Some((pattern, insensitive)) => {
                    let file_name = entry.file_name().to_string_lossy().into_owned();
                    if *insensitive {
                        glob_match(&pattern.to_lowercase(), &file_name.to_lowercase())
                    } else {
                        glob_match(pattern, &file_name)
                    }
                }
                None => true,
            };
            let kind_matches = match kind.as_deref() {
                Some("f") => meta.is_file(),
                Some("d") => meta.is_dir(),
                _ => true,
            };
            if name_matches && kind_matches {
                out.push(relative(workspace, &path));
            }
            if meta.is_dir() && depth < max_depth {
                stack.push((path, depth + 1));
            }
        }
    }
    out.sort();
    Ok(out.join("\n"))
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    if !parts[0].is_empty() {
        if let Some(suffix) = rest.strip_prefix(parts[0]) {
            rest = suffix;
        } else {
            return false;
        }
    }
    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        if let Some(index) = rest.find(part) {
            rest = &rest[index + part.len()..];
        } else {
            return false;
        }
    }
    rest.ends_with(parts[parts.len() - 1])
}

fn stat(workspace: &Path, args: &[String], started: Instant) -> Result<String, String> {
    if args.is_empty() {
        return Err("stat requires a path".to_string());
    }
    let mut out = String::new();
    for raw in args {
        check_time(started)?;
        if raw.starts_with('-') {
            return Err(format!("stat option not allowed: {raw}"));
        }
        let path = resolve(workspace, raw)?;
        let meta = std::fs::symlink_metadata(&path).map_err(|err| err.to_string())?;
        out.push_str(&format!(
            "{}: {} {} bytes\n",
            relative(workspace, &path),
            if meta.is_dir() { "directory" } else if meta.is_file() { "file" } else { "other" },
            meta.len()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_syntax_is_refused() {
        for command in [
            "grep x . | rm -rf /",
            "grep x .; rm -rf /",
            "grep x . && rm -rf /",
            "grep x . > out.txt",
            "grep $(rm -rf /) .",
            "grep `rm -rf /` .",
            "grep x .\nrm -rf /",
        ] {
            assert!(has_unquoted_shell_syntax(command), "{command}");
        }
        assert!(!has_unquoted_shell_syntax("grep -n 'foo|bar' notes.txt"));
    }

    #[test]
    fn destructive_commands_are_not_allowed() {
        for name in DESTRUCTIVE {
            assert!(!ALLOWED.contains(name), "{name} must not be allowlisted");
        }
    }

    #[test]
    fn glob_matching_is_literal_except_star() {
        assert!(glob_match("*.docx", "worksheet.docx"));
        assert!(glob_match("work*", "worksheet.docx"));
        assert!(!glob_match("*.docx", "worksheet.pdf"));
        assert!(glob_match("a*b*c", "axxbyyc"));
    }
}
