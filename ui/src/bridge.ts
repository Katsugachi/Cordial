// The only place the UI touches Tauri.
//
// When the bundle is opened in a normal browser (design previews, UI tests) a
// demo bridge takes over, so the interface can be reviewed without a desktop
// build. The demo is labelled in the header and never writes files.

import type {
  AgentEvent,
  AttachmentInput,
  Bootstrap,
  ModelSummary,
  Session,
  SessionSummary,
  Settings,
} from "./types";
import { DEFAULT_MODEL, DEFAULT_SETTINGS } from "./store";

export interface Bridge {
  readonly demo: boolean;
  bootstrap(): Promise<Bootstrap>;
  sendMessage(sessionId: string, text: string, attachments: AttachmentInput[]): Promise<void>;
  approveTool(callId: string, allow: boolean): Promise<boolean>;
  cancelTurn(): Promise<void>;
  createSession(): Promise<Session>;
  loadSession(id: string): Promise<Session>;
  deleteSession(id: string): Promise<void>;
  listSessions(): Promise<SessionSummary[]>;
  saveSettings(settings: Settings): Promise<ModelSummary>;
  downloadModel(): Promise<void>;
  deleteModel(): Promise<void>;
  pickFiles(): Promise<string[]>;
  pickFolder(): Promise<string | null>;
  openPath(path: string): Promise<void>;
  onEvent(handler: (event: AgentEvent) => void): Promise<() => void>;
}

function hasTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];
const TEXT_EXTENSIONS = ["txt", "md", "markdown", "json", "csv", "log", "rs", "ts", "js", "py", "toml", "yaml", "yml", "html", "css", "docx"];

class TauriBridge implements Bridge {
  readonly demo = false;

  async bootstrap(): Promise<Bootstrap> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<Bootstrap>("bootstrap");
  }

  async sendMessage(sessionId: string, text: string, attachments: AttachmentInput[]): Promise<void> {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("send_message", { sessionId, text, attachments });
  }

  async approveTool(callId: string, allow: boolean): Promise<boolean> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<boolean>("approve_tool", { callId, allow });
  }

  async cancelTurn(): Promise<void> {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("cancel_turn");
  }

  async createSession(): Promise<Session> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<Session>("create_session");
  }

  async loadSession(id: string): Promise<Session> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<Session>("load_session", { id });
  }

  async deleteSession(id: string): Promise<void> {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("delete_session", { id });
  }

  async listSessions(): Promise<SessionSummary[]> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<SessionSummary[]>("list_sessions");
  }

  async saveSettings(settings: Settings): Promise<ModelSummary> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<ModelSummary>("save_settings", { settings });
  }

  async downloadModel(): Promise<void> {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("download_model");
  }

  async deleteModel(): Promise<void> {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("delete_model");
  }

  async pickFiles(): Promise<string[]> {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selection = await open({
      multiple: true,
      directory: false,
      filters: [
        { name: "Images and text", extensions: [...IMAGE_EXTENSIONS, ...TEXT_EXTENSIONS] },
        { name: "Images", extensions: IMAGE_EXTENSIONS },
        { name: "Text files", extensions: TEXT_EXTENSIONS },
      ],
    });
    if (!selection) return [];
    return Array.isArray(selection) ? selection : [selection];
  }

  async pickFolder(): Promise<string | null> {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selection = await open({ multiple: false, directory: true, title: "Choose a working folder" });
    return typeof selection === "string" ? selection : null;
  }

  async openPath(path: string): Promise<void> {
    const { openPath } = await import("@tauri-apps/plugin-opener");
    await openPath(path);
  }

  async onEvent(handler: (event: AgentEvent) => void): Promise<() => void> {
    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<AgentEvent>("cordial://event", (payload) => handler(payload.payload));
    return unlisten;
  }
}

/** A believable, entirely local stand-in used for browser previews and tests. */
class DemoBridge implements Bridge {
  readonly demo = true;
  private listeners: ((event: AgentEvent) => void)[] = [];
  private settings: Settings = { ...DEFAULT_SETTINGS };
  private model: ModelSummary = {
    ...DEFAULT_MODEL,
    status: "ready",
    vision: true,
    backend: "metal",
    description: "LFM3-VL 2B Q4_K_M (demo)",
    missing_bytes: 0,
  };
  private sessions: Session[] = [this.sampleSession()];
  private timers: number[] = [];
  private activeTurn: { turnId: string; finish: () => void } | null = null;

  private sampleSession(): Session {
    const now = Date.now();
    return {
      id: "demo-session",
      title: "Tide times in Manly",
      created_at: now - 86_400_000,
      updated_at: now,
      messages: [
        {
          id: "demo-1",
          role: "user",
          content: "What time is high tide at Manly today?",
          tool_calls: [],
          attachments: [],
          created_at: now - 120_000,
        },
        {
          id: "demo-2",
          role: "assistant",
          content:
            "Example: high tide at 09:42 and 22:06. This is not live data.",
          thinking: "I should check the tide table rather than guess. Searching for the Manly tide chart.",
          tool_calls: [],
          tool_call_id: null,
          attachments: [],
          created_at: now - 100_000,
        },
      ],
    };
  }

  async bootstrap(): Promise<Bootstrap> {
    return {
      settings: this.settings,
      model: this.model,
      sessions: this.sessions.map((session) => this.summary(session)),
      workspace_dir: "/Users/you/Library/Application Support/Cordial/workspace",
      app: { version: "0.1.0-demo", llama_version: "b12020 (demo)", llama_tag: "b12020", backend: "metal" },
      busy: this.activeTurn !== null,
    };
  }

  private summary(session: Session): SessionSummary {
    return {
      id: session.id,
      title: session.title,
      updated_at: session.updated_at,
      message_count: session.messages.length,
    };
  }

  private emit(event: AgentEvent): void {
    for (const listener of this.listeners) listener(event);
  }

  private later(callback: () => void, delay: number): void {
    this.timers.push(window.setTimeout(callback, delay));
  }

  async sendMessage(sessionId: string, text: string, attachments: AttachmentInput[]): Promise<void> {
    const session = this.sessions.find((entry) => entry.id === sessionId);
    if (!session) return;
    const turnId = `demo-turn-${Date.now()}`;
    session.messages.push({
      id: `user-${Date.now()}`,
      role: "user",
      content: text,
      tool_calls: [],
      attachments: attachments.map((attachment) => ({
        kind: attachment.kind,
        name: attachment.path.split(/[\\/]/).pop() ?? attachment.path,
        path: attachment.path,
      })),
      created_at: Date.now(),
    });

    this.emit({ type: "turn_started", turn_id: turnId, session_id: sessionId });

    const wantsSearch = /weather|news|today|latest|price|score|who is|tide|time/i.test(text);
    if (wantsSearch) {
      this.later(() => {
        this.emit({
          type: "tool_call_started",
          turn_id: turnId,
          call_id: "demo-call-1",
          name: "web_search",
          preview: `web_search(query=${text.slice(0, 48)})`,
          risk: "uses the network",
        });
      }, 180);
      this.later(() => {
        this.emit({
          type: "tool_call_finished",
          turn_id: turnId,
          call_id: "demo-call-1",
          name: "web_search",
          ok: true,
          summary: "3 results",
          detail: "1. Bureau of Meteorology\nhttps://www.bom.gov.au\nTide predictions for Sydney.",
          thumbnail: null,
        });
      }, 900);
    }

    const thinking = "Checking what I can verify, then answering in one go.";
    const answer = wantsSearch
      ? `Demo response. No web search was performed.`
      : `Demo response. No model is running.`;

    let index = 0;
    const step = Math.max(12, Math.ceil(answer.length / 40));
    const stream = () => {
      if (index === 0) {
        this.emit({ type: "thinking_delta", turn_id: turnId, text: thinking });
      }
      const chunk = answer.slice(index, index + step);
      index += step;
      if (chunk) this.emit({ type: "text_delta", turn_id: turnId, text: chunk });
      if (index < answer.length) {
        this.later(stream, 45);
      } else {
        const message = {
          id: `assistant-${Date.now()}`,
          role: "assistant" as const,
          content: answer,
          thinking,
          tool_calls: [],
          tool_call_id: null,
          attachments: [],
          created_at: Date.now(),
        };
        session.messages.push(message);
        this.emit({ type: "assistant_message", turn_id: turnId, message });
        this.emit({
          type: "turn_finished",
          turn_id: turnId,
          stop: "answered",
          rounds: wantsSearch ? 2 : 1,
          prompt_tokens: 812,
          output_tokens: 140,
          tokens_per_second: 24.5,
        });
        this.activeTurn?.finish();
        this.activeTurn = null;
      }
    };
    await new Promise<void>((resolve) => {
      this.activeTurn = { turnId, finish: resolve };
      this.later(stream, wantsSearch ? 1100 : 320);
    });
  }

  async approveTool(): Promise<boolean> {
    return true;
  }

  async cancelTurn(): Promise<void> {
    this.timers.forEach((timer) => window.clearTimeout(timer));
    this.timers = [];
    if (this.activeTurn) {
      this.emit({ type: "turn_finished", turn_id: this.activeTurn.turnId, stop: "cancelled", rounds: 0, prompt_tokens: 0, output_tokens: 0, tokens_per_second: 0 });
      this.activeTurn.finish();
      this.activeTurn = null;
    }
  }

  async createSession(): Promise<Session> {
    const session: Session = {
      id: `demo-${Date.now()}`,
      title: "New chat",
      created_at: Date.now(),
      updated_at: Date.now(),
      messages: [],
    };
    this.sessions.push(session);
    return structuredClone(session);
  }

  async loadSession(id: string): Promise<Session> {
    const session = this.sessions.find((entry) => entry.id === id);
    if (!session) throw new Error("Chat not found.");
    return structuredClone(session);
  }

  async deleteSession(id: string): Promise<void> {
    this.sessions = this.sessions.filter((session) => session.id !== id);
  }

  async listSessions(): Promise<SessionSummary[]> {
    return this.sessions.map((session) => this.summary(session));
  }

  async saveSettings(settings: Settings): Promise<ModelSummary> {
    this.settings = settings;
    return this.model;
  }

  async downloadModel(): Promise<void> {
    this.emit({ type: "download_progress", file: "LFM3-VL-2B-Q4_K_M.gguf", received: 0, total: 1_100_000_000 });
  }

  async deleteModel(): Promise<void> {
    this.model = { ...this.model, status: "missing", missing_bytes: 1_303_000_000 };
  }

  async pickFiles(): Promise<string[]> {
    return [];
  }

  async pickFolder(): Promise<string | null> {
    return null;
  }

  async openPath(): Promise<void> {
    // Nothing to open in a browser preview.
  }

  async onEvent(handler: (event: AgentEvent) => void): Promise<() => void> {
    this.listeners.push(handler);
    return () => {
      this.listeners = this.listeners.filter((listener) => listener !== handler);
    };
  }
}

export function createBridge(): Bridge {
  return hasTauri() ? new TauriBridge() : new DemoBridge();
}
