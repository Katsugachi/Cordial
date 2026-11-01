// Application state and the reducer that turns harness events into view state.
//
// The reducer is pure, which is what the UI tests exercise: given a state and an
// event, assert the resulting view.

import type {
  AgentEvent,
  AppInfo,
  Attachment,
  Message,
  ModelSummary,
  Session,
  SessionSummary,
  Settings,
  StopReason,
} from "./types";

export type Phase = "booting" | "missing" | "loading" | "ready" | "demo";

export interface ToolView {
  id: string;
  name: string;
  preview: string;
  risk: string;
  status: "running" | "awaiting" | "done" | "denied";
  summary: string;
  detail: string;
  thumbnail: string | null;
}

export interface TurnView {
  id: string;
  thinking: string;
  text: string;
  tools: ToolView[];
}

export interface DownloadView {
  file: string;
  received: number;
  total: number | null;
}

export interface AppState {
  phase: Phase;
  settings: Settings;
  model: ModelSummary;
  app: AppInfo;
  workspace: string;
  sessions: SessionSummary[];
  session: Session | null;
  turn: TurnView | null;
  approval: ToolView | null;
  download: DownloadView | null;
  downloadError: string | null;
  notice: string | null;
  error: string | null;
  busy: boolean;
}

export const DEFAULT_SETTINGS: Settings = {
  context_tokens: 131072,
  permission_mode: "auto",
  effort: "medium",
  model_path: "",
  mmproj_path: "",
  workspace_dir: "",
  threads: 0,
  web_enabled: true,
  memory: "",
  skill: "",
};

export const DEFAULT_MODEL: ModelSummary = {
  status: "missing",
  description: "",
  context_tokens: 131072,
  backend: "cpu",
  vision: false,
  missing_bytes: 0,
  weights_path: "",
  projector_path: "",
};

export const EMPTY_APP: AppInfo = {
  version: "0.1.0",
  llama_version: "",
  llama_tag: "",
  backend: "cpu",
};

export function initialState(): AppState {
  return {
    phase: "booting",
    settings: { ...DEFAULT_SETTINGS },
    model: { ...DEFAULT_MODEL },
    app: { ...EMPTY_APP },
    workspace: "",
    sessions: [],
    session: null,
    turn: null,
    approval: null,
    download: null,
    downloadError: null,
    notice: null,
    error: null,
    busy: false,
  };
}

/** Phases in which the chat surface, rather than the download screen, is shown. */
export function showsChat(state: AppState): boolean {
  return state.phase === "ready" || state.phase === "demo";
}

export function stopNotice(stop: StopReason): string | null {
  switch (stop) {
    case "tool_budget":
      return "Tool limit reached.";
    case "context_full":
      return "Context full. Start a new chat.";
    case "cancelled":
      return null;
    case "empty_answer":
      return "No answer. Try again or increase effort.";
    case "failed":
      return "Response failed.";
    case "answered":
      return null;
    default:
      return null;
  }
}

function upsertTool(tools: ToolView[], callId: string, update: Partial<ToolView> & { name?: string }): ToolView[] {
  const index = tools.findIndex((tool) => tool.id === callId);
  if (index === -1) {
    const created: ToolView = {
      id: callId,
      name: update.name ?? "tool",
      preview: update.preview ?? "",
      risk: update.risk ?? "",
      status: update.status ?? "running",
      summary: update.summary ?? "",
      detail: update.detail ?? "",
      thumbnail: update.thumbnail ?? null,
    };
    return [...tools, created];
  }
  const next = [...tools];
  next[index] = { ...next[index], ...update };
  return next;
}

function appendMessage(session: Session | null, message: Message): Session | null {
  if (!session) return session;
  if (session.messages.some((existing) => existing.id === message.id)) return session;
  return { ...session, messages: [...session.messages, message] };
}

/** Applies one harness event. Never mutates the input state. */
export function applyEvent(state: AppState, event: AgentEvent): AppState {
  switch (event.type) {
    case "turn_started":
      return {
        ...state,
        turn: { id: event.turn_id, thinking: "", text: "", tools: [] },
        busy: true,
        approval: null,
        error: null,
        notice: null,
      };

    case "generation_reset":
      if (!state.turn || state.turn.id !== event.turn_id) return state;
      return { ...state, turn: { ...state.turn, text: "", thinking: "" } };

    case "thinking_delta":
      if (!state.turn || state.turn.id !== event.turn_id) return state;
      return { ...state, turn: { ...state.turn, thinking: state.turn.thinking + event.text } };

    case "text_delta":
      if (!state.turn || state.turn.id !== event.turn_id) return state;
      return { ...state, turn: { ...state.turn, text: state.turn.text + event.text } };

    case "tool_call_started": {
      if (!state.turn || state.turn.id !== event.turn_id) return state;
      return {
        ...state,
        turn: {
          ...state.turn,
          tools: upsertTool(state.turn.tools, event.call_id, {
            name: event.name,
            preview: event.preview,
            risk: event.risk,
            status: "running",
          }),
        },
      };
    }

    case "tool_call_needs_approval": {
      const tool: ToolView = {
        id: event.call_id,
        name: event.name,
        preview: event.preview,
        risk: event.risk,
        status: "awaiting",
        summary: "",
        detail: "",
        thumbnail: null,
      };
      const tools = state.turn
        ? upsertTool(state.turn.tools, event.call_id, { ...tool })
        : [tool];
      return {
        ...state,
        turn: state.turn ? { ...state.turn, tools } : state.turn,
        approval: tool,
      };
    }

    case "tool_call_finished": {
      if (!state.turn) return state;
      return {
        ...state,
        turn: {
          ...state.turn,
          tools: upsertTool(state.turn.tools, event.call_id, {
            name: event.name,
            status: event.ok ? "done" : "denied",
            summary: event.summary,
            detail: event.detail,
            thumbnail: event.thumbnail ?? null,
          }),
        },
        approval: state.approval?.id === event.call_id ? null : state.approval,
      };
    }

    case "tool_call_denied": {
      if (!state.turn) return state;
      return {
        ...state,
        turn: {
          ...state.turn,
          tools: upsertTool(state.turn.tools, event.call_id, {
            name: event.name,
            status: "denied",
            summary: "Denied by you",
          }),
        },
        approval: state.approval?.id === event.call_id ? null : state.approval,
      };
    }

    case "assistant_message": {
      const session = appendMessage(state.session, event.message);
      // Text now lives in the transcript; keep the live tool cards until the
      // matching tool messages arrive.
      return {
        ...state,
        session,
        turn: state.turn ? { ...state.turn, text: "", thinking: "" } : state.turn,
      };
    }

    case "tool_message": {
      const session = appendMessage(state.session, event.message);
      const tools = state.turn
        ? state.turn.tools.filter((tool) => tool.id !== event.message.tool_call_id)
        : [];
      return {
        ...state,
        session,
        turn: state.turn ? { ...state.turn, tools } : state.turn,
      };
    }

    case "turn_finished":
      return {
        ...state,
        turn: null,
        approval: null,
        busy: false,
        notice: stopNotice(event.stop) ?? state.notice,
      };

    case "error":
      return { ...state, error: event.message, busy: false, turn: null, approval: null };

    case "notice":
      return { ...state, notice: event.message };

    case "model_loading":
      return { ...state, phase: "loading" };

    case "model_ready":
      return {
        ...state,
        phase: "ready",
        model: {
          ...state.model,
          status: "ready",
          description: event.description,
          context_tokens: event.context_tokens,
          backend: event.backend,
          vision: event.vision,
          missing_bytes: 0,
        },
      };

    case "download_progress":
      return {
        ...state,
        download: { file: event.file, received: event.received, total: event.total },
        downloadError: null,
      };

    case "download_finished":
      return { ...state, download: null };

    case "download_failed":
      return { ...state, download: null, downloadError: event.message };

    default:
      return state;
  }
}

/** Adds a file to the composer's attachment strip. */
export function withAttachment(attachments: Attachment[], attachment: Attachment): Attachment[] {
  if (attachments.some((existing) => existing.path === attachment.path)) return attachments;
  return [...attachments, attachment];
}

export function withoutAttachment(attachments: Attachment[], index: number): Attachment[] {
  return attachments.filter((_, position) => position !== index);
}

/** Keeps the newest sessions first. */
export function sortSessions(sessions: SessionSummary[]): SessionSummary[] {
  return [...sessions].sort((a, b) => b.updated_at - a.updated_at);
}
