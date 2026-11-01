//! First-run model download and integrity checks.
//!
//! Cordial ships without weights and fetches Liquid AI's GGUF build the first
//! time the window opens. Downloads resume, are verified against the digest
//! Hugging Face publishes for the file, and stream progress so the UI can show
//! a real progress rail.
//!
//! The digests below are pinned rather than read from response headers: a CDN
//! `ETag` is not necessarily the object's SHA-256 (it was not for these files),
//! and a checksum an attacker controls proves nothing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::Client;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::CoreError;
use crate::events::{AgentEvent, EventSink};

/// Where the default weights live on the Hub.
pub const HF_REPO: &str = "https://huggingface.co/LiquidAI/LFM3-VL-2B-GGUF/resolve/main";

/// Default quantisation: Q4_K_M balances quality, size and speed on laptop CPUs.
pub const DEFAULT_WEIGHTS: &str = "LFM3-VL-2B-Q4_K_M.gguf";
/// Vision projector paired with the Q4_K_M weights.
pub const DEFAULT_PROJECTOR: &str = "mmproj-LFM3-VL-2B-Q8_0.gguf";

/// `LFM3-VL-2B-Q4_K_M.gguf`, from the Hub's file listing of 2026-10-31.
const WEIGHTS_SHA256: &str = "56dac2e192ff0d9ab8507f4f1aeb4aed0463449bbbefaac79497f041c95f21eb";
const WEIGHTS_BYTES: u64 = 1_099_627_808;

/// `mmproj-LFM3-VL-2B-Q8_0.gguf`, from the same listing.
const PROJECTOR_SHA256: &str = "c5c5b58d544a18da919172d2d0368090ea1a4a351b6feb7f8a326a264a5918ab";
const PROJECTOR_BYTES: u64 = 203_031_872;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Weights,
    Projector,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub name: &'static str,
    pub kind: ModelKind,
    pub bytes: u64,
    pub sha256: &'static str,
}

impl ModelFile {
    pub fn url(&self) -> String {
        format!("{HF_REPO}/{}", self.name)
    }
}

/// The files Cordial downloads on first run.
pub fn default_files() -> [ModelFile; 2] {
    [
        ModelFile {
            name: DEFAULT_WEIGHTS,
            kind: ModelKind::Weights,
            bytes: WEIGHTS_BYTES,
            sha256: WEIGHTS_SHA256,
        },
        ModelFile {
            name: DEFAULT_PROJECTOR,
            kind: ModelKind::Projector,
            bytes: PROJECTOR_BYTES,
            sha256: PROJECTOR_SHA256,
        },
    ]
}

/// Resolved on-disk locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPaths {
    pub weights: PathBuf,
    pub projector: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStatus {
    Ready(ModelPaths),
    /// Nothing usable yet; `missing_bytes` is the estimated download size.
    Missing {
        missing_bytes: u64,
    },
    /// Weights are present but the projector is not, so images are unavailable.
    TextOnly(ModelPaths),
}

impl ModelStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, ModelStatus::Ready(_))
    }
}

pub struct ModelManager {
    dir: PathBuf,
    client: Client,
}

impl ModelManager {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, CoreError> {
        // No overall request timeout: the weights file is over a gigabyte and a
        // slow connection is not an error. Only connecting is bounded.
        let client = Client::builder()
            .user_agent("Cordial/0.1 (+https://github.com/Katsugachi/Cordial)")
            .connect_timeout(Duration::from_secs(20))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .map_err(|err| CoreError::Network(err.to_string()))?;
        Ok(Self {
            dir: dir.into(),
            client,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn weights_path(&self) -> PathBuf {
        self.dir.join(DEFAULT_WEIGHTS)
    }

    pub fn projector_path(&self) -> PathBuf {
        self.dir.join(DEFAULT_PROJECTOR)
    }

    /// What is on disk right now.
    pub fn status(&self) -> ModelStatus {
        status_in(&self.dir)
    }

    /// Downloads anything missing. Returns the paths that are now usable.
    pub async fn ensure_defaults(&self, events: &EventSink) -> Result<ModelPaths, CoreError> {
        tokio::fs::create_dir_all(&self.dir).await?;

        let mut weights: Option<PathBuf> = None;
        let mut projector: Option<PathBuf> = None;

        for file in default_files() {
            let target = self.dir.join(file.name);
            if is_complete(&target, file.bytes) {
                match file.kind {
                    ModelKind::Weights => weights = Some(target),
                    ModelKind::Projector => projector = Some(target),
                }
                continue;
            }
            match self.download(&file, events).await {
                Ok(path) => match file.kind {
                    ModelKind::Weights => weights = Some(path),
                    ModelKind::Projector => projector = Some(path),
                },
                Err(err) => {
                    events.emit(AgentEvent::DownloadFailed {
                        file: file.name.to_string(),
                        message: err.to_string(),
                    });
                    if file.kind == ModelKind::Weights {
                        return Err(err);
                    }
                    // A missing projector only costs image understanding.
                    events.emit(AgentEvent::Notice {
                        message: "The vision projector could not be downloaded, so images are \
                                  disabled. Search, browsing and files still work."
                            .to_string(),
                    });
                }
            }
        }

        let weights = weights
            .ok_or_else(|| CoreError::ModelNotReady("the model file is missing".to_string()))?;
        Ok(ModelPaths {
            weights,
            projector: projector.filter(|path| is_complete(path, PROJECTOR_BYTES)),
        })
    }

    /// Downloads one file, resuming across interruptions.
    ///
    /// A gigabyte over a home connection will hiccup: each attempt picks up from
    /// the bytes already on disk, and the finished file is only moved into place
    /// once its length and SHA-256 match the pinned digest.
    async fn download(&self, file: &ModelFile, events: &EventSink) -> Result<PathBuf, CoreError> {
        const ATTEMPTS: u32 = 4;

        let target = self.dir.join(file.name);
        let partial = self.dir.join(format!("{}.part", file.name));

        let mut last_error: Option<CoreError> = None;
        for attempt in 1..=ATTEMPTS {
            match self.download_once(file, &partial, events).await {
                Ok(()) => {
                    tokio::fs::rename(&partial, &target).await?;
                    events.emit(AgentEvent::DownloadFinished {
                        file: file.name.to_string(),
                        path: target.display().to_string(),
                    });
                    return Ok(target);
                }
                Err(err) => {
                    let final_attempt = attempt == ATTEMPTS;
                    last_error = Some(err);
                    if final_attempt {
                        break;
                    }
                    events.emit(AgentEvent::Notice {
                        message: format!(
                            "Download of {} was interrupted (attempt {attempt} of {ATTEMPTS}); \
                             resuming from the bytes already on disk.",
                            file.name
                        ),
                    });
                    tokio::time::sleep(Duration::from_secs(u64::from(attempt) * 3)).await;
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| CoreError::Network(format!("could not download {}", file.name))))
    }

    /// One attempt at fetching the remaining bytes into `partial`.
    async fn download_once(
        &self,
        file: &ModelFile,
        partial: &Path,
        events: &EventSink,
    ) -> Result<(), CoreError> {
        let mut received = match tokio::fs::metadata(partial).await {
            Ok(meta) => meta.len(),
            Err(_) => 0,
        };
        if received >= file.bytes {
            // A previous attempt finished writing but never renamed the file.
            received = 0;
            tokio::fs::remove_file(partial).await.ok();
        }

        let mut request = self.client.get(&file.url());
        if received > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={received}-"));
        }
        let response = request
            .send()
            .await
            .map_err(|err| CoreError::Network(err.to_string()))?;

        if !response.status().is_success() {
            return Err(CoreError::Network(format!(
                "{} returned HTTP {}",
                file.url(),
                response.status()
            )));
        }

        let append = received > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        if !append {
            received = 0;
        }

        let mut handle = if append {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(partial)
                .await?
        } else {
            tokio::fs::File::create(partial).await?
        };

        events.emit(AgentEvent::DownloadProgress {
            file: file.name.to_string(),
            received,
            total: Some(file.bytes),
        });

        // Hash from the start so resumed files still verify, in chunks: a 1 GB
        // weights file must never be read into memory at once.
        let mut hasher = Sha256::new();
        if received > 0 {
            use tokio::io::AsyncReadExt;
            let mut existing = tokio::fs::File::open(partial).await?;
            let mut buffer = vec![0u8; 1024 * 1024];
            loop {
                let read = existing.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
        }

        let mut stream = response.bytes_stream();
        let mut last_emit = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| CoreError::Network(err.to_string()))?;
            handle.write_all(&chunk).await?;
            hasher.update(&chunk);
            received += chunk.len() as u64;
            if last_emit.elapsed() >= Duration::from_millis(250) {
                last_emit = Instant::now();
                events.emit(AgentEvent::DownloadProgress {
                    file: file.name.to_string(),
                    received,
                    total: Some(file.bytes),
                });
            }
        }
        handle.flush().await?;
        handle.sync_all().await?;
        drop(handle);

        if received != file.bytes {
            // Keep the partial file: a later attempt resumes from it.
            return Err(CoreError::Network(format!(
                "{} is incomplete ({} of {} bytes)",
                file.name, received, file.bytes
            )));
        }

        let digest: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if digest != file.sha256 {
            // The bytes are wrong, so resuming from them would never work.
            tokio::fs::remove_file(partial).await.ok();
            return Err(CoreError::Network(format!(
                "{} failed its checksum (expected {}, got {digest}). The download was \
                 corrupted in transit and has been discarded.",
                file.name, file.sha256
            )));
        }

        Ok(())
    }

    /// Removes downloaded weights so the next launch starts clean.
    pub async fn remove_defaults(&self) -> Result<(), CoreError> {
        for file in default_files() {
            let path = self.dir.join(file.name);
            if path.exists() {
                tokio::fs::remove_file(path).await?;
            }
            let partial = self.dir.join(format!("{}.part", file.name));
            if partial.exists() {
                tokio::fs::remove_file(partial).await?;
            }
        }
        Ok(())
    }
}

/// The managed-model status for a folder, without building a client.
pub fn status_in(dir: &Path) -> ModelStatus {
    let files = default_files();
    let weights = dir.join(files[0].name);
    let projector = dir.join(files[1].name);
    match (
        is_complete(&weights, files[0].bytes),
        is_complete(&projector, files[1].bytes),
    ) {
        (true, true) => ModelStatus::Ready(ModelPaths {
            weights,
            projector: Some(projector),
        }),
        (true, false) => ModelStatus::TextOnly(ModelPaths {
            weights,
            projector: None,
        }),
        _ => ModelStatus::Missing {
            missing_bytes: files[0].bytes + files[1].bytes,
        },
    }
}

/// A managed file counts as present when its length matches the pinned size.
/// Anything else (an interrupted write, a truncated copy) is re-fetched.
fn is_complete(path: &Path, expected_bytes: u64) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.len() == expected_bytes)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_files_carry_pinned_digests() {
        let files = default_files();
        assert_eq!(files[0].name, DEFAULT_WEIGHTS);
        assert_eq!(files[0].bytes, 1_099_627_808);
        assert_eq!(files[0].sha256.len(), 64);
        assert!(files[0].sha256.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(files[0].url().starts_with(HF_REPO));
        assert_eq!(files[1].kind, ModelKind::Projector);
        assert_eq!(files[1].bytes, 203_031_872);
    }

    #[test]
    fn missing_status_reports_expected_size() {
        let dir = std::env::temp_dir().join(format!("cordial-mm-{}", crate::util::now_ms()));
        let manager = ModelManager::new(&dir).unwrap();
        match manager.status() {
            ModelStatus::Missing { missing_bytes } => {
                assert_eq!(missing_bytes, WEIGHTS_BYTES + PROJECTOR_BYTES);
            }
            other => panic!("unexpected status: {other:?}"),
        }
    }

    #[test]
    fn truncated_files_are_not_trusted() {
        let dir = std::env::temp_dir().join(format!("cordial-mm2-{}", crate::util::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let manager = ModelManager::new(&dir).unwrap();
        // The right name, the wrong length: this must not count as ready.
        std::fs::write(manager.weights_path(), vec![0u8; 1_200_000]).unwrap();
        assert!(matches!(manager.status(), ModelStatus::Missing { .. }));

        // Correct lengths on both files is the ready state.
        let files = default_files();
        let big = vec![0u8; 1024];
        let _ = big;
        let weights = std::fs::OpenOptions::new()
            .write(true)
            .open(manager.weights_path())
            .unwrap();
        weights.set_len(files[0].bytes).unwrap();
        let projector = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(manager.projector_path())
            .unwrap();
        projector.set_len(files[1].bytes).unwrap();
        assert!(manager.status().is_ready());
        std::fs::remove_dir_all(&dir).ok();
    }
}
