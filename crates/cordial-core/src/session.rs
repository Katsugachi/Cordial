//! Conversation model and on-disk storage.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::util::{now_ms, truncate_chars};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    Image,
    Text,
}

/// A file the user attached to a message. Images become vision input; text
/// files are read into the prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub name: String,
    pub path: String,
}

/// One argument of a model-issued tool call. Order is preserved so the prompt
/// can echo the call back exactly as the model wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolArgument {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Vec<ToolArgument>,
}

impl ToolCall {
    pub fn arg(&self, name: &str) -> Option<&str> {
        self.arguments
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }

    pub fn int(&self, name: &str) -> Option<i64> {
        self.arg(name)?.trim().parse().ok()
    }

    pub fn float(&self, name: &str) -> Option<f64> {
        self.arg(name)?.trim().parse().ok()
    }

    pub fn flag(&self, name: &str) -> bool {
        matches!(
            self.arg(name).map(|v| v.trim().to_ascii_lowercase()),
            Some(v) if v == "true" || v == "1" || v == "yes"
        )
    }

    /// Short "name(key=value)" preview for UI cards and logs.
    pub fn preview(&self) -> String {
        let mut out = self.name.clone();
        let mut parts: Vec<String> = Vec::new();
        for arg in self.arguments.iter().take(3) {
            parts.push(format!("{}={}", arg.name, truncate_chars(&arg.value, 48)));
        }
        if !parts.is_empty() {
            out.push('(');
            out.push_str(&parts.join(", "));
            out.push(')');
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub content: String,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: i64,
}

impl Message {
    pub fn user(content: impl Into<String>, attachments: Vec<Attachment>) -> Self {
        Self {
            id: new_id(),
            role: Role::User,
            content: content.into(),
            thinking: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            attachments,
            created_at: now_ms(),
        }
    }

    pub fn assistant(
        content: impl Into<String>,
        thinking: Option<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        Self {
            id: new_id(),
            role: Role::Assistant,
            content: content.into(),
            thinking,
            tool_calls,
            tool_call_id: None,
            attachments: Vec::new(),
            created_at: now_ms(),
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            role: Role::Tool,
            content: content.into(),
            thinking: None,
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            attachments: Vec::new(),
            created_at: now_ms(),
        }
    }

    /// Number of images this message contributes to the prompt.
    pub fn image_count(&self) -> usize {
        self.attachments
            .iter()
            .filter(|a| a.kind == AttachmentKind::Image)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub messages: Vec<Message>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let now = now_ms();
        Self {
            id: new_id(),
            title: "New chat".to_string(),
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.updated_at = now_ms();
        self.messages.push(message);
    }

    /// Index of the most recent user message.
    pub fn last_user_index(&self) -> Option<usize> {
        self.messages.iter().rposition(|m| m.role == Role::User)
    }

    /// Titles a fresh conversation from its first user message.
    pub fn retitle(&mut self, text: &str) {
        if self.title != "New chat" {
            return;
        }
        let line = text.lines().next().unwrap_or("").trim();
        if line.is_empty() {
            return;
        }
        let mut title = truncate_chars(line, 52);
        if let Some(pos) = title.find("[cut:") {
            title.truncate(pos);
            title = title.trim_end().to_string();
            title.push('…');
        }
        self.title = title;
    }

    /// Drops the oldest completed turn after a context overflow. A "turn" is
    /// everything before the second-to-last user message, so tool-call
    /// exchanges are removed as a unit and the picture stays coherent.
    pub fn trim_oldest_turn(&mut self) -> bool {
        let user_positions: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == Role::User)
            .map(|(i, _)| i)
            .collect();
        if user_positions.len() < 2 {
            return false;
        }
        let cutoff = user_positions[1];
        if cutoff == 0 {
            return false;
        }
        self.messages.drain(0..cutoff);
        true
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub message_count: usize,
}

/// One JSON file per conversation.
#[derive(Debug, Clone)]
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn file_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn save(&self, session: &Session) -> Result<(), CoreError> {
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec_pretty(session)?;
        let final_path = self.file_for(&session.id);
        let tmp = final_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &final_path)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Session, CoreError> {
        let path = self.file_for(id);
        let bytes = std::fs::read(&path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn delete(&self, id: &str) -> Result<(), CoreError> {
        let path = self.file_for(id);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Newest first.
    pub fn list(&self) -> Result<Vec<SessionSummary>, CoreError> {
        let mut out = Vec::new();
        let dir = Path::new(&self.dir);
        if !dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(session) = serde_json::from_slice::<Session>(&bytes) else {
                continue;
            };
            out.push(SessionSummary {
                id: session.id,
                title: session.title,
                updated_at: session.updated_at,
                message_count: session.messages.len(),
            });
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_single_line_and_short() {
        let mut session = Session::new();
        session.retitle("What is the capital of Australia?\nAlso tell me about Canberra.");
        assert_eq!(session.title, "What is the capital of Australia?");
    }

    #[test]
    fn long_titles_are_truncated() {
        let mut session = Session::new();
        session.retitle("x".repeat(200));
        assert!(session.title.chars().count() <= 55);
        assert!(session.title.ends_with('…'));
    }

    #[test]
    fn trimming_removes_whole_oldest_turn() {
        let mut session = Session::new();
        session.push(Message::user("first question", vec![]));
        session.push(Message::tool_result("call-1", "tool output"));
        session.push(Message::assistant("first answer", None, vec![]));
        session.push(Message::user("second question", vec![]));
        assert!(session.trim_oldest_turn());
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "second question");
        // A single remaining turn cannot be trimmed any further.
        assert!(!session.trim_oldest_turn());
    }

    #[test]
    fn tool_call_accessors_are_lenient() {
        let call = ToolCall {
            id: "1".into(),
            name: "browser_click".into(),
            arguments: vec![
                ToolArgument {
                    name: "x".into(),
                    value: " 120 ".into(),
                },
                ToolArgument {
                    name: "y".into(),
                    value: "88".into(),
                },
                ToolArgument {
                    name: "submit".into(),
                    value: "TRUE".into(),
                },
            ],
        };
        assert_eq!(call.int("x"), Some(120));
        assert_eq!(call.int("y"), Some(88));
        assert!(call.flag("submit"));
        assert!(call.preview().starts_with("browser_click(x=120"));
    }

    #[test]
    fn store_round_trips_sessions() {
        let dir = std::env::temp_dir().join(format!("cordial-sessions-{}", now_ms()));
        let store = SessionStore::new(&dir);
        let mut session = Session::new();
        session.push(Message::user("store me", vec![]));
        store.save(&session).unwrap();
        let loaded = store.load(&session.id).unwrap();
        assert_eq!(loaded.messages.len(), 1);
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        store.delete(&session.id).unwrap();
        assert!(store.list().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
