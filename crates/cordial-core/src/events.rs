//! The event stream the harness produces and the UI consumes.
//!
//! Everything the interface needs to know arrives as one of these variants, so
//! the UI never has to poll the harness and the harness never depends on the UI.

use serde::{Deserialize, Serialize};

use crate::session::Message;

/// Why a turn stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model answered without asking for another tool.
    Answered,
    /// The effort budget for tool calls ran out.
    ToolBudget,
    /// The user pressed stop.
    Cancelled,
    /// The context window could not fit the conversation, even after trimming.
    ContextFull,
    /// Something went wrong; see the error event.
    Failed,
    /// The model produced only reasoning and never answered.
    EmptyAnswer,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Answered => "answered",
            StopReason::ToolBudget => "tool_budget",
            StopReason::Cancelled => "cancelled",
            StopReason::ContextFull => "context_full",
            StopReason::Failed => "failed",
            StopReason::EmptyAnswer => "empty_answer",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TurnStarted {
        turn_id: String,
        session_id: String,
    },
    ThinkingDelta {
        turn_id: String,
        text: String,
    },
    TextDelta {
        turn_id: String,
        text: String,
    },
    ToolCallStarted {
        turn_id: String,
        call_id: String,
        name: String,
        preview: String,
        risk: String,
    },
    ToolCallNeedsApproval {
        turn_id: String,
        call_id: String,
        name: String,
        preview: String,
        risk: String,
    },
    ToolCallFinished {
        turn_id: String,
        call_id: String,
        name: String,
        ok: bool,
        summary: String,
        detail: String,
        /// Data URL of the first image the tool produced (browser screenshots),
        /// so the window can show what the model is looking at.
        thumbnail: Option<String>,
    },
    ToolCallDenied {
        turn_id: String,
        call_id: String,
        name: String,
    },
    AssistantMessage {
        turn_id: String,
        message: Message,
    },
    ToolMessage {
        turn_id: String,
        message: Message,
    },
    TurnFinished {
        turn_id: String,
        stop: StopReason,
        rounds: u32,
        prompt_tokens: u32,
        output_tokens: u32,
        tokens_per_second: f32,
    },
    Error {
        turn_id: Option<String>,
        message: String,
    },
    Notice {
        message: String,
    },
    // ---- model lifecycle, raised by the inference worker ----
    ModelLoading {
        path: String,
    },
    ModelReady {
        description: String,
        context_tokens: u32,
        backend: String,
        vision: bool,
    },
    DownloadProgress {
        file: String,
        received: u64,
        total: Option<u64>,
    },
    DownloadFinished {
        file: String,
        path: String,
    },
    DownloadFailed {
        file: String,
        message: String,
    },
}

/// Fan-out handle for [`AgentEvent`]s.
///
/// Cloning is cheap, and a silent sink keeps unit tests free of plumbing.
#[derive(Debug, Clone, Default)]
pub struct EventSink {
    sender: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
}

impl EventSink {
    /// Creates a sink plus the receiving end the UI (or a test) drains.
    pub fn channel() -> (Self, tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            Self {
                sender: Some(sender),
            },
            receiver,
        )
    }

    /// A sink that drops everything.
    pub fn silent() -> Self {
        Self { sender: None }
    }

    pub fn emit(&self, event: AgentEvent) {
        if let Some(sender) = &self.sender {
            // A closed receiver means the window went away; dropping is fine.
            let _ = sender.send(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn events_reach_the_receiver() {
        let (sink, mut receiver) = EventSink::channel();
        sink.emit(AgentEvent::Notice {
            message: "hello".into(),
        });
        let event = receiver.recv().await.unwrap();
        match event {
            AgentEvent::Notice { message } => assert_eq!(message, "hello"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn silent_sink_is_harmless() {
        EventSink::silent().emit(AgentEvent::Notice {
            message: "ignored".into(),
        });
    }

    #[test]
    fn events_serialise_with_a_type_tag() {
        let json = serde_json::to_string(&AgentEvent::TurnStarted {
            turn_id: "t1".into(),
            session_id: "s1".into(),
        })
        .unwrap();
        assert!(json.contains("\"type\":\"turn_started\""));
    }
}
