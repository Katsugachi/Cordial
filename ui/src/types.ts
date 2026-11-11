// Types mirrored from `cordial-core` (serde names must match exactly).

export type PermissionMode = "auto" | "manual";
export type Effort = "low" | "medium" | "high" | "task" | "horizon";

export interface Settings {
  context_tokens: number;
  permission_mode: PermissionMode;
  effort: Effort;
  model_path: string;
  mmproj_path: string;
  workspace_dir: string;
  threads: number;
  web_enabled: boolean;
  memory: string;
  skill: string;
}

export type ModelStatus = "ready" | "loading" | "missing" | "text_only" | "error";

export interface ModelSummary {
  status: ModelStatus;
  description: string;
  context_tokens: number;
  backend: string;
  vision: boolean;
  missing_bytes: number;
  weights_path: string;
  projector_path: string;
}

export interface AppInfo {
  version: string;
  llama_version: string;
  llama_tag: string;
  backend: string;
}

export interface Attachment {
  kind: "image" | "text";
  name: string;
  path: string;
  /** Locally rendered PDF page data, sent only to the desktop backend. */
  inline_data?: string;
}

export interface AttachmentInput {
  kind: "image" | "text";
  path: string;
  name?: string;
  inline_data?: string;
}

export interface ToolArgument {
  name: string;
  value: string;
}

export interface ToolCall {
  id: string;
  name: string;
  arguments: ToolArgument[];
}

export interface Message {
  id: string;
  role: "user" | "assistant" | "tool";
  content: string;
  thinking?: string | null;
  tool_calls: ToolCall[];
  tool_call_id?: string | null;
  tool_status?: "ok" | "failed" | "denied" | "interrupted" | null;
  attachments: Attachment[];
  created_at: number;
}

export type PlanStatus = "pending" | "running" | "done" | "blocked";

export interface PlanStep {
  id: number;
  title: string;
  status: PlanStatus;
  note: string;
}

export interface Session {
  id: string;
  title: string;
  created_at: number;
  updated_at: number;
  messages: Message[];
  plan?: PlanStep[];
}

export interface SessionSummary {
  id: string;
  title: string;
  updated_at: number;
  message_count: number;
}

export interface Bootstrap {
  settings: Settings;
  model: ModelSummary;
  sessions: SessionSummary[];
  workspace_dir: string;
  app: AppInfo;
  busy: boolean;
}

export type StopReason =
  | "answered"
  | "tool_budget"
  | "stalled"
  | "cancelled"
  | "context_full"
  | "failed"
  | "empty_answer";

export type AgentEvent =
  | { type: "turn_started"; turn_id: string; session_id: string }
  | { type: "generation_reset"; turn_id: string }
  | { type: "thinking_delta"; turn_id: string; text: string }
  | { type: "text_delta"; turn_id: string; text: string }
  | {
      type: "tool_call_started";
      turn_id: string;
      call_id: string;
      name: string;
      preview: string;
      risk: string;
    }
  | {
      type: "tool_call_needs_approval";
      turn_id: string;
      call_id: string;
      name: string;
      preview: string;
      risk: string;
    }
  | {
      type: "tool_call_finished";
      turn_id: string;
      call_id: string;
      name: string;
      ok: boolean;
      summary: string;
      detail: string;
      thumbnail?: string | null;
    }
  | { type: "tool_call_denied"; turn_id: string; call_id: string; name: string }
  | { type: "assistant_message"; turn_id: string; message: Message }
  | { type: "tool_message"; turn_id: string; message: Message }
  | { type: "plan_updated"; turn_id: string; steps: PlanStep[] }
  | {
      type: "turn_finished";
      turn_id: string;
      stop: StopReason;
      rounds: number;
      prompt_tokens: number;
      output_tokens: number;
      tokens_per_second: number;
    }
  | { type: "error"; turn_id: string | null; message: string }
  | { type: "notice"; message: string }
  | { type: "model_loading"; path: string }
  | {
      type: "model_ready";
      description: string;
      context_tokens: number;
      backend: string;
      vision: boolean;
    }
  | { type: "download_progress"; file: string; received: number; total: number | null }
  | { type: "download_finished"; file: string; path: string }
  | { type: "download_failed"; file: string; message: string };

export const EFFORTS: Effort[] = ["low", "medium", "high", "task", "horizon"];
export const PERMISSION_MODES: PermissionMode[] = ["auto", "manual"];

export function effortLabel(effort: Effort): string {
  return effort === "low" ? "Low" : effort === "medium" ? "Medium" : effort === "high" ? "High" : "Task";
}

export function permissionLabel(mode: PermissionMode): string {
  return mode === "auto" ? "Auto" : "Manual";
}
