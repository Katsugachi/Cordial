//! Tauri commands: the only way the UI talks to the harness.

use std::path::Path;

use cordial_core::session::{Attachment, AttachmentKind, Session, SessionSummary};
use cordial_core::settings::Settings;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{AppState, ModelSummary};

#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub version: String,
    pub llama_version: String,
    pub llama_tag: String,
    pub backend: String,
}

fn app_info() -> AppInfo {
    AppInfo {
        version: cordial_core::VERSION.to_string(),
        llama_version: cordial_llama::llama_version(),
        llama_tag: cordial_llama::LLAMA_CPP_TAG.to_string(),
        backend: cordial_llama::BACKEND.to_string(),
    }
}

/// Everything the window needs on first paint.
#[derive(Debug, Serialize)]
pub struct Bootstrap {
    pub settings: Settings,
    pub model: ModelSummary,
    pub sessions: Vec<SessionSummary>,
    pub workspace_dir: String,
    pub app: AppInfo,
}

#[tauri::command]
pub async fn bootstrap(state: State<'_, AppState>) -> Result<Bootstrap, String> {
    let (settings, model, workspace_dir) = state.summary().await;
    let sessions = {
        let guard = state.core.lock().await;
        guard.store.list().map_err(|err| err.to_string())?
    };
    Ok(Bootstrap {
        settings,
        model,
        sessions,
        workspace_dir,
        app: app_info(),
    })
}

#[derive(Debug, Deserialize)]
pub struct AttachmentInput {
    pub path: String,
    /// "image" or "text".
    pub kind: String,
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    session_id: String,
    text: String,
    attachments: Vec<AttachmentInput>,
) -> Result<(), String> {
    let attachments = attachments
        .into_iter()
        .map(|input| Attachment {
            kind: if input.kind == "image" {
                AttachmentKind::Image
            } else {
                AttachmentKind::Text
            },
            name: Path::new(&input.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| input.path.clone()),
            path: input.path,
        })
        .collect();
    state.run_turn(session_id, text, attachments).await
}

#[tauri::command]
pub async fn approve_tool(
    state: State<'_, AppState>,
    call_id: String,
    allow: bool,
) -> Result<bool, String> {
    Ok(state.approve(&call_id, allow).await)
}

#[tauri::command]
pub async fn cancel_turn(state: State<'_, AppState>) -> Result<(), String> {
    state.stop().await;
    Ok(())
}

#[tauri::command]
pub async fn create_session(state: State<'_, AppState>) -> Result<Session, String> {
    let guard = state.core.lock().await;
    let session = Session::new();
    guard.store.save(&session).map_err(|err| err.to_string())?;
    Ok(session)
}

#[tauri::command]
pub async fn load_session(state: State<'_, AppState>, id: String) -> Result<Session, String> {
    let guard = state.core.lock().await;
    guard.store.load(&id).map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn delete_session(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let guard = state.core.lock().await;
    guard.store.delete(&id).map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionSummary>, String> {
    let guard = state.core.lock().await;
    guard.store.list().map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn save_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<ModelSummary, String> {
    state.save_settings(settings).await?;
    let (_, model, _) = state.summary().await;
    Ok(model)
}

#[tauri::command]
pub async fn download_model(state: State<'_, AppState>) -> Result<(), String> {
    state.download_model().await
}

#[tauri::command]
pub async fn delete_model(state: State<'_, AppState>) -> Result<(), String> {
    state.delete_model().await
}

#[tauri::command]
pub async fn app_details() -> Result<AppInfo, String> {
    Ok(app_info())
}
