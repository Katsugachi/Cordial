//! Cordial core: the local agentic harness.
//!
//! This crate implements everything that makes Cordial an agent: model
//! management, the tool loop, permissions, effort scheduling and session
//! persistence. It is UI-agnostic and driven through plain events so the
//! same core powers the desktop app and the headless test harness.

pub mod settings;

pub use settings::{Effort, PermissionMode, Settings};
