//! Cordial core: the local agentic harness.
//!
//! This crate is everything that makes Cordial an agent — prompt assembly for
//! LFM3, the tool loop, permissions, effort budgets, model management and the
//! llama.cpp inference worker. It has no UI dependencies: the desktop app drives
//! it through events, and the test suite drives it with a scripted model.

pub mod agent;
pub mod backend;
pub mod error;
pub mod events;
pub mod model_manager;
pub mod paths;
pub mod permissions;
pub mod prompt;
pub mod protocol;
pub mod session;
pub mod settings;
pub mod tools;

#[cfg(feature = "sandbox-python")]
pub mod python_sandbox;
pub mod util;

pub use agent::{Agent, AgentConfig, ApprovalGate, TurnResult};
pub use backend::{
    Backend, EngineInfo, EngineOptions, GenStop, GenerateOutput, GenerateRequest, LlamaBackend,
    ScriptedBackend,
};
pub use error::CoreError;
pub use events::{AgentEvent, EventSink, StopReason};
pub use model_manager::{ModelManager, ModelPaths, ModelStatus};
pub use paths::Paths;
pub use permissions::{Permission, PermissionPolicy};
pub use session::{Attachment, AttachmentKind, Message, Role, Session, SessionStore, ToolCall};
pub use settings::{Effort, PermissionMode, Settings};
pub use tools::ToolRuntime;

/// Version of the harness, surfaced in the About panel and logs.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
