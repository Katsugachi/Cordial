import { describe, expect, it } from "vitest";

import { applyEvent, initialState, showsChat, stopNotice, withAttachment } from "../src/store";
import type { AgentEvent, Message } from "../src/types";

const turnId = "turn-1";

function started(state = initialState()) {
  return applyEvent(state, { type: "turn_started", turn_id: turnId, session_id: "s1" });
}

function assistantMessage(content: string, thinking: string | null = null): Message {
  return {
    id: "m1",
    role: "assistant",
    content,
    thinking,
    tool_calls: [],
    tool_call_id: null,
    attachments: [],
    created_at: 1,
  };
}

describe("turn streaming", () => {
  it("accumulates thinking and answer text", () => {
    let state = started();
    state = applyEvent(state, { type: "thinking_delta", turn_id: turnId, text: "checking" });
    state = applyEvent(state, { type: "text_delta", turn_id: turnId, text: "Hello" });
    state = applyEvent(state, { type: "text_delta", turn_id: turnId, text: " there" });

    expect(state.turn?.thinking).toBe("checking");
    expect(state.turn?.text).toBe("Hello there");
    expect(state.busy).toBe(true);
  });

  it("clears a discarded generation before retrying", () => {
    let state = started();
    state = applyEvent(state, { type: "text_delta", turn_id: turnId, text: '{"query":"test"}' });
    state = applyEvent(state, { type: "generation_reset", turn_id: turnId });
    expect(state.turn?.text).toBe("");
    state = applyEvent(state, { type: "text_delta", turn_id: turnId, text: "Answer" });
    expect(state.turn?.text).toBe("Answer");
  });

  it("ignores deltas from a stale turn", () => {
    const state = started();
    const next = applyEvent(state, { type: "text_delta", turn_id: "other", text: "nope" });
    expect(next.turn?.text).toBe("");
  });

  it("moves the answer into the transcript when the message lands", () => {
    let state = started();
    state = { ...state, session: { id: "s1", title: "t", created_at: 0, updated_at: 0, messages: [] } };
    state = applyEvent(state, { type: "text_delta", turn_id: turnId, text: "partial" });
    state = applyEvent(state, { type: "assistant_message", turn_id: turnId, message: assistantMessage("Final.") });

    expect(state.session?.messages).toHaveLength(1);
    expect(state.turn?.text).toBe("");
  });

  it("finishes cleanly", () => {
    let state = started();
    state = applyEvent(state, {
      type: "turn_finished",
      turn_id: turnId,
      stop: "answered",
      rounds: 1,
      prompt_tokens: 10,
      output_tokens: 5,
      tokens_per_second: 20,
    });
    expect(state.turn).toBeNull();
    expect(state.busy).toBe(false);
    expect(state.notice).toBeNull();
  });

  it("explains a budget stop", () => {
    let state = started();
    state = applyEvent(state, {
      type: "turn_finished",
      turn_id: turnId,
      stop: "tool_budget",
      rounds: 8,
      prompt_tokens: 10,
      output_tokens: 5,
      tokens_per_second: 20,
    });
    expect(state.notice).toBe("Tool limit reached.");
  });
});

describe("permissions", () => {
  it("raises an approval request and clears it when answered", () => {
    let state = started();
    state = applyEvent(state, {
      type: "tool_call_needs_approval",
      turn_id: turnId,
      call_id: "c1",
      name: "write_file",
      preview: "write_file(path=plan.md)",
      risk: "writes local files",
    });
    expect(state.approval?.id).toBe("c1");
    expect(state.turn?.tools[0].status).toBe("awaiting");

    state = applyEvent(state, {
      type: "tool_call_finished",
      turn_id: turnId,
      call_id: "c1",
      name: "write_file",
      ok: true,
      summary: "wrote plan.md",
      detail: "done",
      thumbnail: null,
    });
    expect(state.approval).toBeNull();
    expect(state.turn?.tools[0].status).toBe("done");
  });

  it("marks denials", () => {
    let state = started();
    state = applyEvent(state, {
      type: "tool_call_started",
      turn_id: turnId,
      call_id: "c2",
      name: "read_file",
      preview: "read_file(path=a.txt)",
      risk: "reads local files",
    });
    state = applyEvent(state, { type: "tool_call_denied", turn_id: turnId, call_id: "c2", name: "read_file" });
    expect(state.turn?.tools[0].status).toBe("denied");
  });
});

describe("model lifecycle", () => {
  it("switches to the chat once the model is ready", () => {
    let state = initialState();
    expect(showsChat(state)).toBe(false);
    state = applyEvent(state, { type: "model_loading", path: "/models/x.gguf" });
    expect(state.phase).toBe("loading");
    state = applyEvent(state, {
      type: "model_ready",
      description: "LFM3-VL 2B Q4_K_M",
      context_tokens: 131072,
      backend: "metal",
      vision: true,
    });
    expect(state.phase).toBe("ready");
    expect(showsChat(state)).toBe(true);
    expect(state.model.vision).toBe(true);
  });

  it("tracks download progress", () => {
    let state = initialState();
    state = applyEvent(state, {
      type: "download_progress",
      file: "LFM3-VL-2B-Q4_K_M.gguf",
      received: 100,
      total: 200,
    });
    expect(state.download?.received).toBe(100);
    state = applyEvent(state, {
      type: "download_failed",
      file: "LFM3-VL-2B-Q4_K_M.gguf",
      message: "network down",
    });
    expect(state.download).toBeNull();
    expect(state.downloadError).toBe("network down");
  });

  it("surfaces errors without unlocking the turn before it finishes", () => {
    let state = applyEvent(started(), { type: "error", message: "boom", turn_id: turnId });
    expect(state.error).toBe("boom");
    expect(state.busy).toBe(true);
    state = applyEvent(state, {
      type: "turn_finished", turn_id: turnId, stop: "failed",
      rounds: 0, prompt_tokens: 0, output_tokens: 0, tokens_per_second: 0,
    });
    expect(state.busy).toBe(false);
  });

  it("ignores late errors and completions from an older turn", () => {
    const state = started();
    const staleError = applyEvent(state, { type: "error", message: "old failure", turn_id: "old-turn" });
    expect(staleError).toBe(state);
    const staleFinish = applyEvent(state, {
      type: "turn_finished", turn_id: "old-turn", stop: "answered",
      rounds: 1, prompt_tokens: 1, output_tokens: 1, tokens_per_second: 1,
    });
    expect(staleFinish).toBe(state);
    expect(staleFinish.busy).toBe(true);
    expect(staleFinish.turn?.id).toBe(turnId);
  });

  it("keeps an active turn locked when an unrelated model error arrives", () => {
    const state = applyEvent(started(), { type: "error", message: "model warning", turn_id: null });
    expect(state.error).toBe("model warning");
    expect(state.busy).toBe(true);
    expect(state.turn?.id).toBe(turnId);
  });
});

describe("attachments", () => {
  it("deduplicates by path", () => {
    const first = withAttachment([], { kind: "image", name: "a.png", path: "/tmp/a.png" });
    const second = withAttachment(first, { kind: "image", name: "a.png", path: "/tmp/a.png" });
    expect(second).toHaveLength(1);
  });
});

describe("stop notices", () => {
  it("stays quiet for a normal answer", () => {
    expect(stopNotice("answered")).toBeNull();
    expect(stopNotice("context_full")).toBe("Context full. Start a new chat.");
  });
});

describe("event typing", () => {
  it("serialises with the snake_case tag the harness uses", () => {
    const event: AgentEvent = { type: "notice", message: "hi" };
    expect(event.type).toBe("notice");
  });
});
