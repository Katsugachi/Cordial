//! A minimal Chrome DevTools Protocol driver.
//!
//! Cordial drives the browser the user already has (Chrome, Chromium, Edge or
//! Brave) over the DevTools websocket. That keeps the download small and means
//! pages render with a real engine instead of a toy fetch. The model sees a
//! screenshot plus the page text, and can click by label or by pixel — clicking
//! by label is far more reliable for a 2B model than coordinate guessing.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::error::CoreError;
use crate::util::{now_ms, truncate_chars};

const VIEWPORT_WIDTH: u32 = 1280;
const VIEWPORT_HEIGHT: u32 = 800;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(25);
const PAGE_TEXT_LIMIT: usize = 12_000;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// What a browser step returns: enough for the model to decide what to do next.
#[derive(Debug, Clone)]
pub struct PageSnapshot {
    pub title: String,
    pub url: String,
    pub text: String,
    pub png: Vec<u8>,
    pub viewport: (u32, u32),
}

impl PageSnapshot {
    /// Writes the screenshot next to the other Cordial scratch files and
    /// returns the path that will be handed to the vision encoder.
    pub async fn save_png(&self, dir: &Path) -> Result<PathBuf, CoreError> {
        tokio::fs::create_dir_all(dir).await?;
        let path = dir.join(format!("page-{}.png", now_ms()));
        tokio::fs::write(&path, &self.png).await?;
        Ok(path)
    }
}

pub struct BrowserSession {
    child: tokio::process::Child,
    socket: Socket,
    next_id: i64,
    user_data_dir: PathBuf,
    viewport: (u32, u32),
}

impl BrowserSession {
    /// Starts a headless browser and connects to its first page.
    pub async fn launch(scratch: &Path) -> Result<Self, CoreError> {
        let binary = find_browser().ok_or(CoreError::NoBrowser)?;
        tokio::fs::create_dir_all(scratch).await.ok();
        let user_data_dir = scratch.join(format!("browser-{}", now_ms()));
        tokio::fs::create_dir_all(&user_data_dir).await?;

        let mut child = tokio::process::Command::new(&binary)
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--hide-scrollbars")
            .arg("--mute-audio")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-background-networking")
            .arg("--disable-features=Translate,MediaRouter")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", user_data_dir.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| CoreError::Other(format!("could not start {binary:?}: {err}")))?;

        let port = match wait_for_port(&user_data_dir, &mut child).await {
            Ok(port) => port,
            Err(err) => {
                let _ = child.kill().await;
                return Err(err);
            }
        };

        let socket_url = match page_socket_url(port).await {
            Ok(url) => url,
            Err(err) => {
                let _ = child.kill().await;
                return Err(err);
            }
        };

        let (socket, _response) = connect_async(socket_url.clone())
            .await
            .map_err(|err| CoreError::Other(format!("could not attach to the browser: {err}")))?;

        let mut session = Self {
            child,
            socket,
            next_id: 1,
            user_data_dir,
            viewport: (VIEWPORT_WIDTH, VIEWPORT_HEIGHT),
        };

        session
            .call(
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": VIEWPORT_WIDTH,
                    "height": VIEWPORT_HEIGHT,
                    "deviceScaleFactor": 1,
                    "mobile": false
                }),
            )
            .await?;
        session.call("Page.enable", json!({})).await?;
        session.call("Runtime.enable", json!({})).await?;
        Ok(session)
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, CoreError> {
        let id = self.next_id;
        self.next_id += 1;
        let payload = json!({ "id": id, "method": method, "params": params }).to_string();
        self.socket
            .send(Message::Text(payload.into()))
            .await
            .map_err(|err| CoreError::Other(format!("browser socket closed: {err}")))?;

        let deadline = Instant::now() + COMMAND_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CoreError::Other(format!("browser did not answer {method}")));
            }
            let next = tokio::time::timeout(remaining, self.socket.next())
                .await
                .map_err(|_| CoreError::Other(format!("browser did not answer {method}")))?;
            let Some(message) = next else {
                return Err(CoreError::Other("browser connection ended".to_string()));
            };
            let message = message
                .map_err(|err| CoreError::Other(format!("browser socket error: {err}")))?;
            match message {
                Message::Text(text) => {
                    let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else {
                        continue;
                    };
                    if value.get("id").and_then(Value::as_i64) != Some(id) {
                        continue; // an event we do not care about
                    }
                    if let Some(error) = value.get("error") {
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error");
                        return Err(CoreError::Other(format!("{method} failed: {message}")));
                    }
                    return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                }
                Message::Ping(payload) => {
                    self.socket
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|err| CoreError::Other(err.to_string()))?;
                }
                Message::Close(_) => {
                    return Err(CoreError::Other("browser closed the connection".to_string()))
                }
                _ => {}
            }
        }
    }

    async fn evaluate(&mut self, expression: &str) -> Result<Value, CoreError> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        if let Some(exception) = result.get("exceptionDetails") {
            let text = exception
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("script error");
            return Err(CoreError::Other(format!("page script failed: {text}")));
        }
        Ok(result
            .get("result")
            .and_then(|inner| inner.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// Waits until the document reports `complete`, then lets the page settle.
    async fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            let state = self
                .evaluate("document.readyState")
                .await
                .unwrap_or(Value::Null);
            if state.as_str() == Some("complete") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    /// Navigates and returns a fresh snapshot.
    pub async fn navigate(&mut self, url: &str) -> Result<PageSnapshot, CoreError> {
        let target = normalise(url)?;
        let result = self
            .call("Page.navigate", json!({ "url": target }))
            .await?;
        if let Some(error) = result.get("errorText").and_then(Value::as_str) {
            return Err(CoreError::Other(format!("could not load {target}: {error}")));
        }
        self.wait_until_ready().await;
        self.snapshot().await
    }

    /// Fresh screenshot plus page text.
    pub async fn snapshot(&mut self) -> Result<PageSnapshot, CoreError> {
        let meta = self
            .evaluate(
                "JSON.stringify({title: document.title || '', url: location.href, \
                 text: (document.body ? document.body.innerText : '')})",
            )
            .await?;
        let parsed: Value = meta
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_else(|| json!({ "title": "", "url": "", "text": "" }));

        let screenshot = self
            .call("Page.captureScreenshot", json!({ "format": "png", "captureBeyondViewport": false }))
            .await?;
        let data = screenshot
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::Other("browser returned no screenshot".to_string()))?;
        let png = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
            .map_err(|err| CoreError::Other(format!("screenshot decode failed: {err}")))?;

        Ok(PageSnapshot {
            title: parsed
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            url: parsed
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            text: truncate_chars(
                parsed.get("text").and_then(Value::as_str).unwrap_or(""),
                PAGE_TEXT_LIMIT,
            ),
            png,
            viewport: self.viewport,
        })
    }

    /// Clicks an element found by its visible label.
    pub async fn click_text(&mut self, needle: &str) -> Result<PageSnapshot, CoreError> {
        let encoded = serde_json::to_string(needle).unwrap_or_else(|_| "\"\"".to_string());
        let script = format!(
            r#"(() => {{
                const needle = {encoded};
                const target = needle.trim().toLowerCase();
                const candidates = Array.from(document.querySelectorAll(
                    'a,button,input[type=submit],input[type=button],[role=button],[role=link],summary,label'
                ));
                const hit = candidates.find((node) => {{
                    const label = (node.innerText || node.value || node.getAttribute('aria-label') || '')
                        .trim().toLowerCase();
                    return label && label.includes(target);
                }});
                if (!hit) return JSON.stringify({{ ok: false }});
                hit.scrollIntoView({{ block: 'center' }});
                hit.click();
                return JSON.stringify({{ ok: true, label: (hit.innerText || '').trim().slice(0, 80) }});
            }})()"#
        );
        let result = self.evaluate(&script).await?;
        let parsed: Value = result
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_else(|| json!({ "ok": false }));
        if parsed.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(CoreError::Other(format!(
                "no clickable element matched “{needle}”. Take a screenshot and try \
                 pixel coordinates instead."
            )));
        }
        self.wait_until_ready().await;
        self.snapshot().await
    }

    /// Clicks at viewport pixel coordinates.
    pub async fn click(&mut self, x: f64, y: f64) -> Result<PageSnapshot, CoreError> {
        let base = json!({
            "x": x,
            "y": y,
            "button": "left",
            "clickCount": 1
        });
        self.call(
            "Input.dispatchMouseEvent",
            merge(base.clone(), json!({ "type": "mouseMoved" })),
        )
        .await?;
        self.call(
            "Input.dispatchMouseEvent",
            merge(base.clone(), json!({ "type": "mousePressed", "buttons": 1 })),
        )
        .await?;
        self.call(
            "Input.dispatchMouseEvent",
            merge(base, json!({ "type": "mouseReleased", "buttons": 0 })),
        )
        .await?;
        self.wait_until_ready().await;
        self.snapshot().await
    }

    /// Focuses an input by label, types, and optionally presses Enter.
    pub async fn type_text(
        &mut self,
        text: &str,
        field: Option<&str>,
        submit: bool,
    ) -> Result<PageSnapshot, CoreError> {
        if let Some(field) = field {
            let encoded = serde_json::to_string(field).unwrap_or_else(|_| "\"\"".to_string());
            let script = format!(
                r#"(() => {{
                    const needle = {encoded}.trim().toLowerCase();
                    const inputs = Array.from(document.querySelectorAll('input,textarea,[contenteditable=true]'));
                    const hit = inputs.find((node) => {{
                        const haystack = [node.getAttribute('placeholder'), node.getAttribute('name'),
                            node.getAttribute('aria-label'), node.id, node.type]
                            .filter(Boolean).join(' ').toLowerCase();
                        return haystack.includes(needle);
                    }});
                    if (!hit) return false;
                    hit.focus();
                    if ('value' in hit) hit.value = '';
                    return true;
                }})()"#
            );
            let focused = self.evaluate(&script).await?;
            if focused.as_bool() != Some(true) {
                return Err(CoreError::Other(format!(
                    "no input matched “{field}”. Click the field first, then type."
                )));
            }
        }

        self.call("Input.insertText", json!({ "text": text })).await?;
        if submit {
            self.press_enter().await?;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        self.wait_until_ready().await;
        self.snapshot().await
    }

    async fn press_enter(&mut self) -> Result<(), CoreError> {
        let key = json!({
            "key": "Enter",
            "code": "Enter",
            "windowsVirtualKeyCode": 13,
            "nativeVirtualKeyCode": 13,
            "text": "\r"
        });
        self.call(
            "Input.dispatchKeyEvent",
            merge(key.clone(), json!({ "type": "rawKeyDown" })),
        )
        .await?;
        self.call(
            "Input.dispatchKeyEvent",
            merge(key.clone(), json!({ "type": "char" })),
        )
        .await?;
        self.call("Input.dispatchKeyEvent", merge(key, json!({ "type": "keyUp" })))
            .await?;
        Ok(())
    }

    /// Scrolls and returns a fresh snapshot. `direction` is "up" or "down".
    pub async fn scroll(&mut self, direction: &str) -> Result<PageSnapshot, CoreError> {
        let delta = match direction.trim().to_ascii_lowercase().as_str() {
            "up" => -600.0,
            _ => 600.0,
        };
        self.call(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseWheel",
                "x": VIEWPORT_WIDTH / 2,
                "y": VIEWPORT_HEIGHT / 2,
                "deltaX": 0,
                "deltaY": delta
            }),
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.snapshot().await
    }

    pub async fn shutdown(mut self) {
        let _ = self.socket.close(None).await;
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        tokio::fs::remove_dir_all(&self.user_data_dir).await.ok();
    }
}

fn merge(mut base: Value, extra: Value) -> Value {
    if let (Some(base_map), Some(extra_map)) = (base.as_object_mut(), extra.as_object()) {
        for (key, value) in extra_map {
            base_map.insert(key.clone(), value.clone());
        }
    }
    base
}

fn normalise(url: &str) -> Result<String, CoreError> {
    let trimmed = url.trim();
    let candidate = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else if trimmed.contains('.') && !trimmed.contains(' ') {
        format!("https://{trimmed}")
    } else {
        return Err(CoreError::Other(format!(
            "“{trimmed}” is not a web address"
        )));
    };
    let parsed = url::Url::parse(&candidate)
        .map_err(|err| CoreError::Other(format!("invalid address: {err}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(CoreError::Other(
            "only http and https addresses can be opened".to_string(),
        ));
    }
    Ok(parsed.to_string())
}

/// Reads the port Chrome picked from `<user-data-dir>/DevToolsActivePort`.
async fn wait_for_port(dir: &Path, child: &mut tokio::process::Child) -> Result<u16, CoreError> {
    let port_file = dir.join("DevToolsActivePort");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(contents) = tokio::fs::read_to_string(&port_file).await {
            if let Some(line) = contents.lines().next() {
                if let Ok(port) = line.trim().parse::<u16>() {
                    return Ok(port);
                }
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(CoreError::Other(format!(
                "the browser exited immediately ({status})"
            )));
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    Err(CoreError::Other(
        "the browser did not open a debugging port in time".to_string(),
    ))
}

/// Asks the DevTools HTTP endpoint for the first page target.
async fn page_socket_url(port: u16) -> Result<String, CoreError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|err| CoreError::Network(err.to_string()))?;

    // Newer Chrome requires PUT for /json/new.
    let _ = client
        .put(format!("http://127.0.0.1:{port}/json/new?about:blank"))
        .send()
        .await;

    let response = client
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await
        .map_err(|err| CoreError::Network(err.to_string()))?;
    let targets: Vec<Value> = response
        .json()
        .await
        .map_err(|err| CoreError::Network(err.to_string()))?;

    targets
        .iter()
        .find(|target| target.get("type").and_then(Value::as_str) == Some("page"))
        .and_then(|target| target.get("webSocketDebuggerUrl"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CoreError::Other("the browser opened no page".to_string()))
}

/// Locates a Chromium-family browser without bundling one.
pub fn find_browser() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("CORDIAL_BROWSER") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        for app in [
            "Google Chrome",
            "Chromium",
            "Microsoft Edge",
            "Brave Browser",
        ] {
            candidates.push(PathBuf::from(format!(
                "/Applications/{app}.app/Contents/MacOS/{app}"
            )));
        }
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        for base in [
            std::env::var("ProgramFiles").ok(),
            std::env::var("ProgramFiles(x86)").ok(),
            std::env::var("LocalAppData").ok(),
        ]
        .into_iter()
        .flatten()
        {
            for relative in [
                "Google/Chrome/Application/chrome.exe",
                "Microsoft/Edge/Application/msedge.exe",
                "Chromium/Application/chrome.exe",
                "BraveSoftware/Brave-Browser/Application/brave.exe",
            ] {
                candidates.push(PathBuf::from(&base).join(relative));
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        for name in [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "brave-browser",
        ] {
            if let Some(path) = which(name) {
                candidates.push(path);
            }
        }
    }

    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(target_os = "linux")]
fn which(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalised() {
        assert_eq!(normalise("example.com").unwrap(), "https://example.com/");
        assert!(normalise("not an address").is_err());
        assert!(normalise("file:///etc/passwd").is_err());
    }

    #[test]
    fn clicks_are_merged_with_coordinates() {
        let merged = merge(
            json!({ "x": 10.0, "y": 20.0, "button": "left" }),
            json!({ "type": "mousePressed" }),
        );
        assert_eq!(merged["type"], "mousePressed");
        assert_eq!(merged["x"], 10.0);
    }

    #[test]
    fn find_browser_returns_an_existing_path() {
        if let Some(path) = find_browser() {
            assert!(path.is_file());
        }
    }
}
