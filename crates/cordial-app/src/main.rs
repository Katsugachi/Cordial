#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Cordial: a local agent powered by LFM3-VL-2B through llama.cpp.

mod commands;
mod pdf_attachments;
mod state;

use state::AppState;
use tauri::Manager;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cordial=info,warn".into()),
        )
        .with_target(false)
        .init();

    // llama.cpp logs go to tracing so they land in the same place.
    cordial_llama::set_logger(|level, message| match level {
        cordial_llama::LogLevel::Error => tracing::error!(target: "llama", "{message}"),
        cordial_llama::LogLevel::Warn => tracing::warn!(target: "llama", "{message}"),
        cordial_llama::LogLevel::Info => tracing::info!(target: "llama", "{message}"),
        _ => tracing::debug!(target: "llama", "{message}"),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            match AppState::new(handle) {
                Ok(state) => {
                    state.boot_in_background();
                    app.manage(state);
                }
                Err(err) => {
                    tracing::error!("could not start Cordial: {err}");
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::send_message,
            commands::approve_tool,
            commands::cancel_turn,
            commands::create_session,
            commands::load_session,
            commands::delete_session,
            commands::list_sessions,
            commands::save_settings,
            commands::download_model,
            commands::delete_model,
            commands::app_details,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Cordial");
}
