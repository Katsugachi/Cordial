//! Adversarial tests for the confined command runner.
//!
//! The runner is not a shell. These tests check that destructive commands,
//! shell syntax, path escapes and symlink escapes are refused, and that the
//! allowed read-only commands still work inside the workspace.

use std::path::PathBuf;

use cordial_core::session::{ToolArgument, ToolCall};
use cordial_core::settings::Effort;
use cordial_core::tools::ToolRuntime;

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "cordial-cmd-{tag}-{}-{}",
            std::process::id(),
            cordial_core::util::now_ms()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("notes.txt"), "alpha\nTODO: ship\nbeta\n").unwrap();
        std::fs::create_dir_all(workspace.join("sub")).unwrap();
        std::fs::write(workspace.join("sub/more.md"), "TODO: later\n").unwrap();
        Self { root, workspace }
    }

    fn runtime(&self) -> ToolRuntime {
        ToolRuntime::new(
            self.workspace.clone(),
            self.root.join("scratch"),
            Effort::Medium,
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn call(command: &str) -> ToolCall {
    ToolCall {
        id: "c1".into(),
        name: "run_command".into(),
        arguments: vec![ToolArgument { name: "command".into(), value: command.into() }],
    }
}

#[tokio::test]
async fn destructive_commands_are_refused_and_have_no_effect() {
    let fixture = Fixture::new("destructive");
    let runtime = fixture.runtime();
    let commands = [
        "rm -rf .",
        "rm -rf /",
        "rmdir sub",
        "del notes.txt",
        "erase notes.txt",
        "Remove-Item notes.txt",
        "mv notes.txt other.txt",
        "cp notes.txt other.txt",
        "chmod 777 notes.txt",
        "dd if=/dev/zero of=notes.txt",
        "sudo rm -rf .",
        "bash -c 'rm -rf .'",
        "sh -c 'rm -rf .'",
        "powershell -Command Remove-Item",
        "cmd /c del notes.txt",
        "python -c 'import os; os.remove(\"notes.txt\")'",
        "node -e 'require(\"fs\").unlinkSync(\"notes.txt\")'",
        "curl https://example.com",
        "wget https://example.com",
        "git clean -fdx",
        "npm install",
        "cargo build",
        "shutdown now",
        "kill 1",
        "touch new.txt",
        "mkdir new",
        "ln -s /etc/passwd link",
        "tee notes.txt",
        "sed -i s/a/b/ notes.txt",
        "find . -delete",
        "find . -exec rm {} ;",
    ];
    for command in commands {
        let outcome = runtime.execute(&call(command)).await;
        assert!(!outcome.ok, "command was allowed: {command}\n{}", outcome.text);
        assert!(fixture.workspace.join("notes.txt").exists(), "command changed files: {command}");
    }
}

#[tokio::test]
async fn shell_syntax_is_refused() {
    let fixture = Fixture::new("shell");
    let runtime = fixture.runtime();
    for command in [
        "grep TODO notes.txt | wc -l",
        "grep TODO notes.txt; rm -rf .",
        "grep TODO notes.txt && rm -rf .",
        "grep TODO notes.txt || rm -rf .",
        "grep TODO notes.txt > out.txt",
        "grep TODO notes.txt >> out.txt",
        "cat < notes.txt",
        "echo $(rm -rf .)",
        "echo `rm -rf .`",
        "grep TODO notes.txt\nrm -rf .",
        "grep TODO notes.txt &",
    ] {
        let outcome = runtime.execute(&call(command)).await;
        assert!(!outcome.ok, "shell syntax was allowed: {command}");
    }
}

#[tokio::test]
async fn path_escapes_are_refused() {
    let fixture = Fixture::new("escape");
    let runtime = fixture.runtime();
    std::fs::write(fixture.root.join("outside.txt"), "secret").unwrap();
    for command in [
        "cat ../outside.txt",
        "cat ../../etc/passwd",
        "cat /etc/passwd",
        "grep secret ../outside.txt",
        "ls ..",
        "find .. -name outside.txt",
        "stat ../outside.txt",
    ] {
        let outcome = runtime.execute(&call(command)).await;
        assert!(!outcome.ok, "path escape was allowed: {command}\n{}", outcome.text);
        assert!(!outcome.text.contains("secret"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_is_refused() {
    let fixture = Fixture::new("symlink");
    let runtime = fixture.runtime();
    let outside = fixture.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.workspace.join("link")).unwrap();
    for command in ["cat link/secret.txt", "grep secret link", "ls link", "find link -type f"] {
        let outcome = runtime.execute(&call(command)).await;
        assert!(!outcome.ok || !outcome.text.contains("secret"), "symlink escape: {command}\n{}", outcome.text);
    }
}

#[tokio::test]
async fn read_only_commands_work_inside_workspace() {
    let fixture = Fixture::new("allowed");
    let runtime = fixture.runtime();
    let cases = [
        ("pwd", "workspace"),
        ("ls", "notes.txt"),
        ("cat notes.txt", "TODO: ship"),
        ("head -n 1 notes.txt", "alpha"),
        ("tail -n 1 notes.txt", "beta"),
        ("wc -l notes.txt", "3"),
        ("grep -n TODO notes.txt", "2:TODO: ship"),
        ("grep -r TODO .", "sub/more.md"),
        ("find . -name *.md", "sub/more.md"),
        ("stat notes.txt", "file"),
    ];
    for (command, expected) in cases {
        let outcome = runtime.execute(&call(command)).await;
        assert!(outcome.ok, "allowed command failed: {command}\n{}", outcome.text);
        assert!(outcome.text.contains(expected), "command {command} returned {}", outcome.text);
    }
}

#[tokio::test]
async fn quoted_regex_metacharacters_are_not_shell_syntax() {
    let fixture = Fixture::new("regex");
    let runtime = fixture.runtime();
    let outcome = runtime.execute(&call("grep -E 'TODO|alpha' notes.txt")).await;
    assert!(outcome.ok, "{}", outcome.text);
    assert!(outcome.text.contains("alpha"));
    assert!(outcome.text.contains("TODO"));
}
