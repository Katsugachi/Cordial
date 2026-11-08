//! Application state: settings, model lifecycle and turn execution.
//!
//! One mutex guards what a turn needs (engine, tools, policy, store) while the
//! model manager, approval gate and cancel flag stay outside it, so Stop and
//! Approve always respond even while the model is generating.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cordial_core::backend::{engine_options, Backend, EngineInfo, LlamaBackend};
use cordial_core::events::{AgentEvent, EventSink};
use cordial_core::model_manager::{self, ModelManager, ModelPaths, ModelStatus};
use cordial_core::permissions::PermissionPolicy;
use cordial_core::session::{Attachment, Session, SessionStore};
use cordial_core::settings::Settings;
use cordial_core::tools::ToolRuntime;
use cordial_core::{Agent, AgentConfig, ApprovalGate, CoreError, Paths};
use cordial_llama::SamplingParams;
#[cfg(feature = "experimental-python")]
use tauri::Manager;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

/// Event name the frontend listens on.
pub const EVENT_NAME: &str = "cordial://event";

pub struct Engine {
    pub backend: Backend,
    pub paths: ModelPaths,
    pub info: EngineInfo,
}

/// Releases the turn lock on every exit, including a cancelled command future.
struct TurnGuard<'a> {
    busy: &'a AtomicBool,
}

impl<'a> TurnGuard<'a> {
    fn acquire(busy: &'a AtomicBool) -> Option<Self> {
        if busy.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(Self { busy })
        }
    }
}

impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::SeqCst);
    }
}

pub struct Core {
    pub paths: Paths,
    pub settings: Settings,
    pub store: SessionStore,
    pub tools: Arc<ToolRuntime>,
    pub policy: PermissionPolicy,
    pub engine: Option<Engine>,
    pub model_load_error: Option<String>,
    permission_session_id: Option<String>,
    python_path: Option<PathBuf>,
}

fn select_permission_session(
    policy: &mut PermissionPolicy,
    current: &mut Option<String>,
    session_id: &str,
) {
    if current.as_deref() != Some(session_id) {
        policy.clear_grants();
        *current = Some(session_id.to_string());
    }
}

fn create_tools(
    workspace: PathBuf,
    scratch: PathBuf,
    effort: cordial_core::settings::Effort,
    python_path: Option<&Path>,
) -> Result<ToolRuntime, CoreError> {
    let tools = ToolRuntime::new(workspace, scratch, effort)?;
    #[cfg(feature = "experimental-python")]
    let tools = if let Some(path) = python_path {
        tools.with_python_path(path.to_path_buf())
    } else {
        tools
    };
    #[cfg(not(feature = "experimental-python"))]
    let _ = python_path;
    Ok(tools)
}

impl Core {
    fn load(python_path: Option<PathBuf>) -> Result<Self, CoreError> {
        let paths = match Paths::discover() {
            Ok(paths) => paths,
            Err(_) => Paths::at(std::env::temp_dir().join("Cordial")),
        };
        paths.ensure()?;

        let settings = read_settings(&paths);
        let store = SessionStore::new(paths.sessions_dir());
        let workspace = workspace_for(&paths, &settings);
        std::fs::create_dir_all(&workspace)?;
        let tools = create_tools(
            workspace,
            paths.root().join("scratch"),
            settings.effort,
            python_path.as_deref(),
        )?;

        Ok(Self {
            policy: PermissionPolicy::new(settings.permission_mode),
            settings,
            store,
            tools: Arc::new(tools),
            engine: None,
            model_load_error: None,
            permission_session_id: None,
            python_path,
            paths,
        })
    }

    /// The weights and projector that should be loaded right now.
    fn model_paths(&self) -> Result<(PathBuf, Option<PathBuf>), CoreError> {
        let models_dir = self.paths.models_dir();
        let weights = if !self.settings.model_path.trim().is_empty()
            && Path::new(self.settings.model_path.trim()).is_file()
        {
            PathBuf::from(self.settings.model_path.trim())
        } else {
            match model_manager::status_in(&models_dir) {
                ModelStatus::Ready(paths) | ModelStatus::TextOnly(paths) => paths.weights,
                ModelStatus::Missing { .. } => {
                    return Err(CoreError::ModelNotReady(
                        "the model file is missing or incomplete".to_string(),
                    ))
                }
            }
        };
        if !weights.is_file() {
            return Err(CoreError::ModelNotReady(
                "the LFM3-VL-2B model has not been downloaded yet".to_string(),
            ));
        }
        let projector = if !self.settings.mmproj_path.trim().is_empty()
            && Path::new(self.settings.mmproj_path.trim()).is_file()
        {
            Some(PathBuf::from(self.settings.mmproj_path.trim()))
        } else {
            match model_manager::status_in(&models_dir) {
                ModelStatus::Ready(paths) => paths.projector,
                _ => None,
            }
        };
        Ok((weights, projector))
    }

    fn is_model_ready(&self) -> bool {
        self.engine.is_some() || self.model_paths().is_ok()
    }

    async fn boot_engine(
        &mut self,
        events: &EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<(), CoreError> {
        self.model_load_error = None;
        let (weights, projector) = self.model_paths()?;
        let sampling = SamplingParams {
            temperature: self.settings.effort.temperature(),
            ..SamplingParams::default()
        };
        let options = engine_options(
            &weights,
            projector.as_deref(),
            self.settings.context_tokens,
            self.settings.threads as i32,
            sampling,
        );
        let (backend, ready) = LlamaBackend::spawn(options, events.clone(), cancel)?;
        let info = ready.await.map_err(|_| {
            CoreError::Inference("the inference worker stopped while loading".to_string())
        })??;
        self.engine = Some(Engine {
            backend: Backend::Llama(backend),
            paths: ModelPaths { weights, projector },
            info,
        });
        Ok(())
    }
}

/// Everything the Tauri commands share.
pub struct AppState {
    pub core: Arc<Mutex<Core>>,
    pub models: Arc<ModelManager>,
    pub events: EventSink,
    pub approvals: ApprovalGate,
    pub cancel: Arc<AtomicBool>,
    pub busy: AtomicBool,
    pub downloading: AtomicBool,
}

impl AppState {
    pub fn new(app: AppHandle) -> Result<Self, String> {
        #[cfg(feature = "experimental-python")]
        let python_path = app
            .path()
            .resolve("python/python.wasm", tauri::path::BaseDirectory::Resource)
            .ok()
            .filter(|path| path.is_file())
            .or_else(|| {
                if cfg!(debug_assertions) {
                    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../ui/node_modules/@antonz/python-wasi/dist/python.wasm");
                    path.is_file().then_some(path)
                } else {
                    None
                }
            });
        #[cfg(not(feature = "experimental-python"))]
        let python_path = None;
        let (events, mut receiver) = EventSink::channel();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = receiver.recv().await {
                let _ = app.emit(EVENT_NAME, &event);
            }
        });

        let core = Core::load(python_path).map_err(|err| err.to_string())?;
        let models = ModelManager::new(core.paths.models_dir()).map_err(|err| err.to_string())?;
        Ok(Self {
            core: Arc::new(Mutex::new(core)),
            models: Arc::new(models),
            events,
            approvals: ApprovalGate::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: AtomicBool::new(false),
            downloading: AtomicBool::new(false),
        })
    }

    /// Loads the model in the background once the window is up.
    pub fn boot_in_background(&self) {
        let core = Arc::clone(&self.core);
        let events = self.events.clone();
        let cancel = Arc::clone(&self.cancel);
        tauri::async_runtime::spawn(async move {
            let mut guard = core.lock().await;
            if !guard.is_model_ready() {
                drop(guard);
                events.emit(AgentEvent::Notice {
                    message: "The local model is not downloaded yet.".to_string(),
                });
                return;
            }
            if let Err(err) = guard.boot_engine(&events, cancel).await {
                guard.model_load_error = Some(err.to_string());
                events.emit(AgentEvent::Error {
                    turn_id: None,
                    message: err.to_string(),
                });
            }
        });
    }

    /// Runs one user turn, saving the session afterwards.
    pub async fn run_turn(
        &self,
        session_id: String,
        text: String,
        attachments: Vec<crate::commands::AttachmentInput>,
    ) -> Result<(), String> {
        let Some(_guard) = TurnGuard::acquire(&self.busy) else {
            return Err("Cordial is still answering the previous message.".to_string());
        };
        self.cancel.store(false, Ordering::Relaxed);
        let scratch = {
            let core = self.core.lock().await;
            core.paths.root().join("scratch").join("pdf-attachments")
        };
        let attachments = crate::pdf_attachments::prepare(attachments, &scratch)?;
        self.run_turn_inner(session_id, text, attachments).await
    }

    async fn run_turn_inner(
        &self,
        session_id: String,
        text: String,
        attachments: Vec<Attachment>,
    ) -> Result<(), String> {
        let mut guard = self.core.lock().await;
        let Core {
            engine,
            tools,
            policy,
            permission_session_id,
            store,
            settings,
            ..
        } = &mut *guard;

        let Some(engine) = engine.as_mut() else {
            return Err("The model is not ready yet. Wait for it to finish loading.".to_string());
        };
        select_permission_session(policy, permission_session_id, &session_id);

        let mut session = store.load(&session_id).unwrap_or_else(|_| {
            let mut session = Session::new();
            session.id = session_id;
            session
        });

        let config = AgentConfig::from_settings(settings);
        let result = {
            let mut agent = Agent {
                backend: &mut engine.backend,
                tools: tools.as_ref(),
                policy,
                approvals: &self.approvals,
                events: &self.events,
                config,
                store: Some(store),
            };
            agent.run(&mut session, text, attachments).await
        };

        store.save(&session).map_err(|err| err.to_string())?;
        result.map(|_| ()).map_err(|err| err.to_string())
    }

    pub async fn approve(&self, call_id: &str, allow: bool) -> bool {
        self.approvals.resolve(call_id, allow).await
    }

    pub async fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.approvals.deny_all().await;
    }

    /// Downloads the default model files, then loads them.
    pub async fn download_model(&self) -> Result<(), String> {
        if self.downloading.swap(true, Ordering::SeqCst) {
            return Err("A download is already running.".to_string());
        }
        let outcome = self.download_model_inner().await;
        self.downloading.store(false, Ordering::SeqCst);
        outcome
    }

    async fn download_model_inner(&self) -> Result<(), String> {
        self.models
            .ensure_defaults(&self.events)
            .await
            .map_err(|err| err.to_string())?;

        let mut guard = self.core.lock().await;
        match guard
            .boot_engine(&self.events, Arc::clone(&self.cancel))
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                guard.model_load_error = Some(err.to_string());
                Err(err.to_string())
            }
        }
    }

    pub async fn delete_model(&self) -> Result<(), String> {
        {
            let mut guard = self.core.lock().await;
            guard.engine = None;
        }
        self.models
            .remove_defaults()
            .await
            .map_err(|err| err.to_string())
    }

    pub async fn save_settings(&self, next: Settings) -> Result<Settings, String> {
        let mut guard = self.core.lock().await;
        let previous = guard.settings.clone();
        let settings = next.sanitised(128 * 1024);
        write_settings(&guard.paths, &settings).map_err(|err| err.to_string())?;
        guard.settings = settings.clone();
        guard.policy.set_mode(settings.permission_mode);
        if previous.workspace_dir != settings.workspace_dir {
            guard.policy.clear_grants();
        }

        if previous.workspace_dir != settings.workspace_dir || previous.effort != settings.effort {
            let workspace = workspace_for(&guard.paths, &settings);
            std::fs::create_dir_all(&workspace).map_err(|err| err.to_string())?;
            guard.tools = Arc::new(
                create_tools(
                    workspace,
                    guard.paths.root().join("scratch"),
                    settings.effort,
                    guard.python_path.as_deref(),
                )
                .map_err(|err| err.to_string())?,
            );
        }

        let reload = previous.context_tokens != settings.context_tokens
            || previous.model_path != settings.model_path
            || previous.mmproj_path != settings.mmproj_path
            || previous.threads != settings.threads;
        if reload {
            // Dropping the engine stops the worker thread and frees the weights.
            guard.engine = None;
            if guard.model_paths().is_ok() {
                guard
                    .boot_engine(&self.events, Arc::clone(&self.cancel))
                    .await
                    .map_err(|err| err.to_string())?;
            }
        }
        Ok(settings)
    }

    pub async fn summary(&self) -> (Settings, ModelSummary, String) {
        let guard = self.core.lock().await;
        let workspace = workspace_for(&guard.paths, &guard.settings);
        (
            guard.settings.clone(),
            model_summary(&guard),
            workspace.display().to_string(),
        )
    }
}

/// Snapshot handed to the UI on first paint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelSummary {
    pub status: String,
    pub description: String,
    pub context_tokens: u32,
    pub backend: String,
    pub vision: bool,
    pub missing_bytes: u64,
    pub weights_path: String,
    pub projector_path: String,
}

pub fn model_summary(core: &Core) -> ModelSummary {
    let models_dir = core.paths.models_dir();
    let status = model_manager::status_in(&models_dir);

    if let Some(engine) = &core.engine {
        return ModelSummary {
            status: "ready".to_string(),
            description: engine.info.description.clone(),
            context_tokens: engine.info.context_tokens,
            backend: engine.info.backend.clone(),
            vision: engine.info.vision,
            missing_bytes: 0,
            weights_path: engine.paths.weights.display().to_string(),
            projector_path: engine
                .paths
                .projector
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        };
    }

    if core.model_load_error.is_none() && core.model_paths().is_ok() {
        return ModelSummary {
            status: "loading".to_string(),
            description: String::new(),
            context_tokens: core.settings.context_tokens,
            backend: cordial_llama::BACKEND.to_string(),
            vision: core
                .model_paths()
                .ok()
                .and_then(|(_, projector)| projector)
                .is_some(),
            missing_bytes: 0,
            weights_path: core
                .model_paths()
                .map(|(weights, _)| weights.display().to_string())
                .unwrap_or_default(),
            projector_path: String::new(),
        };
    }

    let (name, missing_bytes, vision) = match &status {
        ModelStatus::Ready(paths) => ("loading", 0, paths.projector.is_some()),
        ModelStatus::TextOnly(_) => {
            let files = model_manager::default_files();
            ("text_only", files[1].bytes, false)
        }
        ModelStatus::Missing { missing_bytes } => ("missing", *missing_bytes, false),
    };

    ModelSummary {
        status: if core.model_load_error.is_some() {
            "error"
        } else {
            name
        }
        .to_string(),
        description: String::new(),
        context_tokens: core.settings.context_tokens,
        backend: cordial_llama::BACKEND.to_string(),
        vision,
        missing_bytes,
        weights_path: models_dir
            .join(model_manager::DEFAULT_WEIGHTS)
            .display()
            .to_string(),
        projector_path: models_dir
            .join(model_manager::DEFAULT_PROJECTOR)
            .display()
            .to_string(),
    }
}

pub fn workspace_for(paths: &Paths, settings: &Settings) -> PathBuf {
    if settings.workspace_dir.trim().is_empty() {
        paths.workspace_dir()
    } else {
        PathBuf::from(settings.workspace_dir.trim())
    }
}

pub fn read_settings(paths: &Paths) -> Settings {
    let file = paths.settings_file();
    let Ok(bytes) = std::fs::read(&file) else {
        return Settings::default().sanitised(128 * 1024);
    };
    match serde_json::from_slice::<Settings>(&bytes) {
        Ok(settings) => settings.sanitised(128 * 1024),
        Err(_) => Settings::default().sanitised(128 * 1024),
    }
}

pub fn write_settings(paths: &Paths, settings: &Settings) -> Result<(), CoreError> {
    std::fs::create_dir_all(paths.root())?;
    let json = serde_json::to_vec_pretty(settings)?;
    let file = paths.settings_file();
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "experimental-python")]
    #[test]
    fn optional_python_runtime_is_retained_when_tools_are_recreated() {
        let root =
            std::env::temp_dir().join(format!("cordial-app-python-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let module = root.join("python.wasm");
        std::fs::write(&module, b"untrusted; the sandbox will verify before use").unwrap();
        let first = create_tools(
            root.clone(),
            root.join("scratch"),
            cordial_core::settings::Effort::Task,
            Some(&module),
        )
        .unwrap();
        let updated = create_tools(
            root.clone(),
            root.join("scratch"),
            cordial_core::settings::Effort::High,
            Some(&module),
        )
        .unwrap();
        let missing = create_tools(
            root.clone(),
            root.join("scratch"),
            cordial_core::settings::Effort::Task,
            None,
        )
        .unwrap();
        assert!(first.has_python_sandbox() && updated.has_python_sandbox());
        assert!(!missing.has_python_sandbox());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auto_grants_are_not_shared_between_conversations() {
        use cordial_core::settings::PermissionMode;
        let mut policy = PermissionPolicy::new(PermissionMode::Auto);
        let mut current = None;
        select_permission_session(&mut policy, &mut current, "first");
        policy.grant("write_file");
        select_permission_session(&mut policy, &mut current, "first");
        assert!(policy.is_granted("write_file"));
        select_permission_session(&mut policy, &mut current, "second");
        assert!(!policy.is_granted("write_file"));
        policy.grant("docx_edit");
        select_permission_session(&mut policy, &mut current, "first");
        assert!(!policy.is_granted("docx_edit"));
        assert!(!policy.is_granted("write_file"));
    }

    #[test]
    fn turn_guard_admits_one_owner_and_releases_on_drop() {
        let busy = AtomicBool::new(false);
        let guard = TurnGuard::acquire(&busy).expect("first turn acquires the lock");
        assert!(TurnGuard::acquire(&busy).is_none());
        drop(guard);
        assert!(!busy.load(Ordering::SeqCst));
        assert!(TurnGuard::acquire(&busy).is_some());
    }

    #[tokio::test]
    async fn cancelled_turn_future_releases_the_lock() {
        let busy = AtomicBool::new(false);
        let mut future = Box::pin(async {
            let _guard = TurnGuard::acquire(&busy).expect("turn acquires the lock");
            std::future::pending::<()>().await;
        });
        tokio::select! {
            biased;
            _ = &mut future => unreachable!("pending future cannot finish"),
            _ = tokio::task::yield_now() => {}
        }
        assert!(busy.load(Ordering::SeqCst));
        drop(future);
        assert!(!busy.load(Ordering::SeqCst));
        assert!(TurnGuard::acquire(&busy).is_some());
    }
}
