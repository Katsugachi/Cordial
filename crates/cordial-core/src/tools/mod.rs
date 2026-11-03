//! Tools the model can call, and the runtime that executes them.
//!
//! Tool names and schemas are deliberately small and flat: LFM3-VL-2B is a 2B
//! model, and every extra property is a chance for a malformed call. Each tool
//! declares a [`Risk`] which the permission layer uses to decide whether the
//! user must approve the call first.

pub mod browser;
pub mod commands;
pub mod docx;
pub mod files;
pub mod htmltext;
pub mod web;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio::sync::Mutex;

use crate::error::CoreError;
use crate::session::ToolCall;
use crate::settings::Effort;

/// How dangerous a tool is, from the user's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Reads local data that the user already shared.
    Read,
    /// Reads from the network.
    Network,
    /// Changes local files.
    Write,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Read => "reads local files",
            Risk::Network => "uses the network",
            Risk::Write => "writes local files",
        }
    }
}

/// A tool definition as shown to the model and the UI.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
    pub risk: Risk,
}

impl ToolSpec {
    /// Renders the definition the way the LFM3 chat template expects:
    /// `{"name": ..., "description": ..., "parameters": {...}}`.
    pub fn to_prompt_json(&self) -> String {
        let name = serde_json::to_string(self.name).unwrap_or_else(|_| "\"\"".into());
        let description = serde_json::to_string(self.description).unwrap_or_else(|_| "\"\"".into());
        let parameters = serde_json::to_string(&self.parameters).unwrap_or_else(|_| "{}".into());
        format!(
            "{{\"name\": {name}, \"description\": {description}, \"parameters\": {parameters}}}"
        )
    }
}

fn string_param(description: &str) -> serde_json::Value {
    json!({ "type": "string", "description": description })
}

/// The full tool list. Web tools are omitted when networking is turned off.
pub fn specs(web_enabled: bool) -> Vec<ToolSpec> {
    let mut out = Vec::new();

    if web_enabled {
        out.push(ToolSpec {
            name: "web_search",
            description: "Search the web. Returns titles, links and snippets.",
            parameters: json!({
                "type": "object",
                "properties": { "query": string_param("Search keywords") },
                "required": ["query"]
            }),
            risk: Risk::Network,
        });
        out.push(ToolSpec {
            name: "web_fetch",
            description: "Open a web page and return its readable text.",
            parameters: json!({
                "type": "object",
                "properties": { "url": string_param("Full http or https link") },
                "required": ["url"]
            }),
            risk: Risk::Network,
        });
        out.push(ToolSpec {
            name: "browser_open",
            description: "Open a page in a headless browser. Returns a screenshot and page text.",
            parameters: json!({
                "type": "object",
                "properties": { "url": string_param("Full http or https link") },
                "required": ["url"]
            }),
            risk: Risk::Network,
        });
        out.push(ToolSpec {
            name: "browser_click",
            description: "Click a link or button by its label, or click a pixel.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": string_param("Visible label to click"),
                    "x": { "type": "integer", "description": "Horizontal pixel" },
                    "y": { "type": "integer", "description": "Vertical pixel" }
                }
            }),
            risk: Risk::Network,
        });
        out.push(ToolSpec {
            name: "browser_type",
            description: "Type into a page field, optionally pressing Enter.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": string_param("Text to type"),
                    "field": string_param("Placeholder, name or label of the field"),
                    "submit": { "type": "boolean", "description": "Press Enter afterwards" }
                },
                "required": ["text"]
            }),
            risk: Risk::Network,
        });
        out.push(ToolSpec {
            name: "browser_view",
            description: "Screenshot and text of the current page. Scrolls if asked.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "scroll": string_param("up or down, optional")
                }
            }),
            risk: Risk::Network,
        });
    }

    out.push(ToolSpec {
        name: "read_file",
        description: "Read a text file from the workspace.",
        parameters: json!({
            "type": "object",
            "properties": { "path": string_param("Path relative to the workspace") },
            "required": ["path"]
        }),
        risk: Risk::Read,
    });
    out.push(ToolSpec {
        name: "list_files",
        description: "List files in a workspace folder.",
        parameters: json!({
            "type": "object",
            "properties": { "path": string_param("Folder relative to the workspace, or .") }
        }),
        risk: Risk::Read,
    });
    out.push(ToolSpec {
        name: "write_file",
        description: "Write text to a file in the workspace. Asks the user first.",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": string_param("Path relative to the workspace"),
                "content": string_param("Full file contents")
            },
            "required": ["path", "content"]
        }),
        risk: Risk::Write,
    });

    out.push(ToolSpec {
        name: "run_command",
        description: "Run one read-only workspace command: grep, find, ls, cat, head, tail, wc, stat or pwd. No shell, writes, network or destructive commands.",
        parameters: json!({
            "type": "object",
            "properties": { "command": string_param("A single command, for example: grep -n TODO notes.txt") },
            "required": ["command"]
        }),
        risk: Risk::Read,
    });
    out.push(ToolSpec {
        name: "docx_read",
        description: "Read a DOCX file as numbered paragraphs [p0], [p1], ...",
        parameters: json!({
            "type": "object",
            "properties": { "path": string_param("DOCX path relative to the workspace") },
            "required": ["path"]
        }),
        risk: Risk::Read,
    });
    out.push(ToolSpec {
        name: "docx_edit",
        description: "Replace selected paragraphs in a DOCX copy. edits is a JSON array of {paragraph, text}.",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": string_param("Source DOCX path"),
                "edits": string_param("JSON array, for example [{\"paragraph\":1,\"text\":\"Answer\"}]"),
                "output": string_param("Optional output DOCX path; defaults to <name>.edited.docx")
            },
            "required": ["path", "edits"]
        }),
        risk: Risk::Write,
    });
    out.push(ToolSpec {
        name: "docx_create",
        description: "Create a simple DOCX file from plain text lines.",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": string_param("Output DOCX path"),
                "content": string_param("Plain text; each line becomes a paragraph")
            },
            "required": ["path", "content"]
        }),
        risk: Risk::Write,
    });
    out.push(ToolSpec {
        name: "update_plan",
        description: "Create or update the visible task checklist. steps is a JSON array of {title, status, note}. status: pending, running, done, blocked.",
        parameters: json!({
            "type": "object",
            "properties": { "steps": string_param("JSON array of checklist steps") },
            "required": ["steps"]
        }),
        risk: Risk::Read,
    });

    out
}

/// Looks up a spec by name.
pub fn spec_for(name: &str) -> Option<ToolSpec> {
    specs(true).into_iter().find(|s| s.name == name)
}

/// What a tool produced.
#[derive(Debug, Clone, Default)]
pub struct ToolOutcome {
    pub ok: bool,
    /// Text handed back to the model inside `<tool_response>`.
    pub text: String,
    /// Images produced by the tool, attached to the following model turn.
    pub media: Vec<PathBuf>,
    /// One-line description for the UI card.
    pub summary: String,
}

impl ToolOutcome {
    pub fn ok(summary: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
            media: Vec::new(),
            summary: summary.into(),
        }
    }

    pub fn fail(summary: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
            media: Vec::new(),
            summary: summary.into(),
        }
    }

    pub fn with_media(mut self, media: Vec<PathBuf>) -> Self {
        self.media = media;
        self
    }
}

/// Executes tools for one conversation.
pub struct ToolRuntime {
    http: reqwest::Client,
    workspace: PathBuf,
    scratch: PathBuf,
    effort: Effort,
    browser: Arc<Mutex<Option<browser::BrowserSession>>>,
}

impl ToolRuntime {
    pub fn new(workspace: PathBuf, scratch: PathBuf, effort: Effort) -> Result<Self, CoreError> {
        let http = reqwest::Client::builder()
            .user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/131.0 Safari/537.36 Cordial/0.1",
            )
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(8))
            .build()
            .map_err(|e| CoreError::Network(e.to_string()))?;
        Ok(Self {
            http,
            workspace,
            scratch,
            effort,
            browser: Arc::new(Mutex::new(None)),
        })
    }

    pub fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn effort(&self) -> Effort {
        self.effort
    }

    /// Runs a tool call. Errors are returned as failed outcomes so the model can
    /// read them and try something else, which is far more useful than a crash.
    pub async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        match call.name.as_str() {
            "web_search" => web::search(self, call).await,
            "web_fetch" => web::fetch(self, call).await,
            "browser_open" => self.browser_open(call).await,
            "browser_click" => self.browser_click(call).await,
            "browser_type" => self.browser_type(call).await,
            "browser_view" => self.browser_view(call).await,
            "read_file" => files::read(self, call),
            "list_files" => files::list(self, call),
            "write_file" => files::write(self, call),
            "run_command" => commands::run(self, call).await,
            "docx_read" => docx::read(self, call),
            "docx_edit" => docx::edit(self, call),
            "docx_create" => docx::create(self, call),
            "update_plan" => ToolOutcome::fail("plan handled by agent", "Error: update_plan must be handled by the agent loop."),
            other => ToolOutcome::fail(
                format!("unknown tool: {other}"),
                format!(
                    "There is no tool called `{other}`. Available tools: {}.",
                    specs(true)
                        .iter()
                        .map(|s| s.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
        }
    }

    async fn browser_open(&self, call: &ToolCall) -> ToolOutcome {
        let Some(url) = call.arg("url") else {
            return ToolOutcome::fail("missing url", "Error: `url` is required.");
        };
        let mut guard = self.browser.lock().await;
        if guard.is_none() {
            match browser::BrowserSession::launch(&self.scratch).await {
                Ok(session) => *guard = Some(session),
                Err(err) => {
                    return ToolOutcome::fail(
                        "no browser available",
                        format!(
                            "Error: {err}\nUse `web_fetch` with a direct link instead, \
                             or ask the user to install Chrome, Chromium or Edge."
                        ),
                    );
                }
            }
        }
        let session = guard.as_mut().expect("browser session just created");
        match session.navigate(url.trim()).await {
            Ok(snapshot) => self.browser_outcome(snapshot).await,
            Err(err) => ToolOutcome::fail("browser navigation failed", format!("Error: {err}")),
        }
    }

    async fn browser_click(&self, call: &ToolCall) -> ToolOutcome {
        let label = call
            .arg("text")
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let point = match (call.float("x"), call.float("y")) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        };
        if label.is_none() && point.is_none() {
            return ToolOutcome::fail(
                "missing target",
                "Error: give either `text` (the label to click) or both `x` and `y`.",
            );
        }

        let mut guard = self.browser.lock().await;
        let Some(session) = guard.as_mut() else {
            return ToolOutcome::fail(
                "no open page",
                "Error: no page is open. Call `browser_open` first.",
            );
        };
        let result = match (label, point) {
            (Some(label), _) => session.click_text(label).await,
            (None, Some((x, y))) => session.click(x, y).await,
            (None, None) => unreachable!("validated above"),
        };
        match result {
            Ok(snapshot) => self.browser_outcome(snapshot).await,
            Err(err) => ToolOutcome::fail("click failed", format!("Error: {err}")),
        }
    }

    async fn browser_type(&self, call: &ToolCall) -> ToolOutcome {
        let Some(text) = call.arg("text") else {
            return ToolOutcome::fail("missing text", "Error: `text` is required.");
        };
        let submit = call.flag("submit");
        let field = call.arg("field").map(str::trim).filter(|f| !f.is_empty());
        let mut guard = self.browser.lock().await;
        let Some(session) = guard.as_mut() else {
            return ToolOutcome::fail(
                "no open page",
                "Error: no page is open. Call `browser_open` first.",
            );
        };
        match session.type_text(text, field, submit).await {
            Ok(snapshot) => self.browser_outcome(snapshot).await,
            Err(err) => ToolOutcome::fail("typing failed", format!("Error: {err}")),
        }
    }

    async fn browser_view(&self, call: &ToolCall) -> ToolOutcome {
        let mut guard = self.browser.lock().await;
        let Some(session) = guard.as_mut() else {
            return ToolOutcome::fail(
                "no open page",
                "Error: no page is open. Call `browser_open` first.",
            );
        };
        let scroll = call
            .arg("scroll")
            .map(str::trim)
            .filter(|direction| !direction.is_empty());
        let result = match scroll {
            Some(direction) => session.scroll(direction).await,
            None => session.snapshot().await,
        };
        match result {
            Ok(snapshot) => self.browser_outcome(snapshot).await,
            Err(err) => ToolOutcome::fail("snapshot failed", format!("Error: {err}")),
        }
    }

    async fn browser_outcome(&self, snapshot: browser::PageSnapshot) -> ToolOutcome {
        let media = match snapshot.save_png(&self.scratch.join("screenshots")).await {
            Ok(path) => vec![path],
            Err(err) => {
                return ToolOutcome::fail("screenshot could not be saved", format!("Error: {err}"))
            }
        };
        let text = format!(
            "Page: {}\nURL: {}\n\n{}",
            snapshot.title,
            snapshot.url,
            crate::util::truncate_chars(&snapshot.text, self.effort.fetch_chars())
        );
        ToolOutcome::ok(
            format!(
                "{}: {}x{}",
                snapshot.title, snapshot.viewport.0, snapshot.viewport.1
            ),
            text,
        )
        .with_media(media)
    }

    /// Closes the headless browser, if one was started.
    pub async fn shutdown(&self) {
        let mut guard = self.browser.lock().await;
        if let Some(session) = guard.take() {
            session.shutdown().await;
        }
    }

    /// Reads a user attachment as text (used for file inputs).
    pub fn read_attachment_text(&self, path: &std::path::Path, max_chars: usize) -> Option<String> {
        if path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.eq_ignore_ascii_case("docx")).unwrap_or(false) {
            return docx::attachment_text(path, max_chars).ok();
        }
        files::read_text_bounded(path, max_chars).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spec_renders_valid_json() {
        for spec in specs(true) {
            let rendered = spec.to_prompt_json();
            let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
            assert_eq!(parsed["name"], spec.name);
            assert!(parsed["parameters"]["type"] == "object");
        }
    }

    #[test]
    fn web_tools_are_optional() {
        let without = specs(false);
        assert!(without.iter().all(|s| s.risk != Risk::Network));
        assert!(specs(true).iter().any(|s| s.name == "web_search"));
    }

    #[test]
    fn write_tool_is_marked_as_write() {
        let spec = spec_for("write_file").unwrap();
        assert_eq!(spec.risk, Risk::Write);
    }
}
