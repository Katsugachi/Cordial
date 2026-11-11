// Integration tests for the whole window: mount, boot, send, stream, approve.
//
// The demo bridge stands in for Tauri, so this exercises the same code paths the
// desktop app uses without needing a webview or a model.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { mount } from "../src/app";
import { createBridge } from "../src/bridge";
import type { AgentEvent } from "../src/types";

async function bootApp(): Promise<HTMLElement> {
  document.body.innerHTML = '<div id="app"></div>';
  const root = document.getElementById("app") as HTMLElement;
  mount(root, createBridge());
  // Let the bootstrap promise chain settle.
  await vi.advanceTimersByTimeAsync(20);
  return root;
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("window", () => {
  it("boots into the demo and shows the sidebar, demo label and transcript", async () => {
    const root = await bootApp();
    expect(root.querySelector(".sidebar")).not.toBeNull();
    expect(root.querySelector(".topbar__title")?.textContent).toBe("Tide times in Manly");
    expect(root.textContent).toContain("Demo");
    expect(root.textContent).toContain("high tide at Manly");
    expect(root.querySelector("#input")).not.toBeNull();
  });

  it("streams a tool call and an answer when you send a message", async () => {
    const root = await bootApp();
    const input = root.querySelector<HTMLTextAreaElement>("#input") as HTMLTextAreaElement;
    input.value = "What is the tide time today?";

    root.querySelector<HTMLElement>('[data-action="send"]')?.click();
    await vi.advanceTimersByTimeAsync(60);

    // The turn started and the search card appeared.
    expect(root.textContent).toContain("Search");

    await vi.advanceTimersByTimeAsync(4000);

    expect(root.textContent).toContain("09:42");
    expect(root.querySelector('[data-streaming="true"]')).toBeNull();
    expect(root.querySelectorAll(".msg").length).toBeGreaterThanOrEqual(4);
  });

  it("shows the stop control while answering and clears it afterwards", async () => {
    const root = await bootApp();
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Tell me something";
    root.querySelector<HTMLElement>('[data-action="send"]')?.click();
    await vi.advanceTimersByTimeAsync(60);

    expect(root.querySelector('[data-action="stop"]')).not.toBeNull();

    await vi.advanceTimersByTimeAsync(5000);
    expect(root.querySelector('[data-action="stop"]')).toBeNull();
    expect(root.querySelector('[data-action="send"]')).not.toBeNull();
  });

  it("changes effort in the composer", async () => {
    const root = await bootApp();
    const select = root.querySelector<HTMLSelectElement>("#effort-select")!;
    select.value = "horizon";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelector<HTMLSelectElement>("#effort-select")?.value).toBe("horizon");
  });

  it("lets the user disable web access and warns before re-enabling it", async () => {
    const root = await bootApp();
    const web = () => root.querySelector<HTMLButtonElement>('[data-action="toggle-web"]')!;
    expect(web().getAttribute("aria-pressed")).toBe("false");
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    web().click();
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining("private network"));
    expect(web().getAttribute("aria-pressed")).toBe("false");
    confirm.mockReturnValue(true);
    web().click();
    await vi.advanceTimersByTimeAsync(20);
    expect(web().getAttribute("aria-pressed")).toBe("true");
    web().click();
    await vi.advanceTimersByTimeAsync(20);
    expect(web().getAttribute("aria-pressed")).toBe("false");
    confirm.mockRestore();
  });

  it("does not change web access during an active turn", async () => {
    const root = await bootApp();
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Hello";
    root.querySelector<HTMLElement>('[data-action="send"]')!.click();
    await vi.advanceTimersByTimeAsync(60);
    const button = root.querySelector<HTMLButtonElement>('[data-action="toggle-web"]')!;
    expect(button.disabled).toBe(true);
    button.click();
    expect(confirm).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(5000);
    expect(root.querySelector<HTMLButtonElement>('[data-action="toggle-web"]')?.getAttribute("aria-pressed")).toBe("false");
    confirm.mockRestore();
  });

  it("saves and reopens memory and skills", async () => {
    const root = await bootApp();
    for (const kind of ["memory", "skill"]) {
      root.querySelector<HTMLElement>(`[data-action="open-${kind}"]`)?.click();
      const input = document.querySelector<HTMLTextAreaElement>("#editor-input")!;
      input.value = `Saved ${kind}`;
      document.querySelector<HTMLElement>('[data-action="save-editor"]')?.click();
      await vi.advanceTimersByTimeAsync(20);
      root.querySelector<HTMLElement>(`[data-action="open-${kind}"]`)?.click();
      expect(document.querySelector<HTMLTextAreaElement>("#editor-input")?.value).toBe(`Saved ${kind}`);
      document.querySelector<HTMLElement>('[data-action="close-editor"]')?.click();
    }
  });

  it("imports a SKILL.md file as editable instructions", async () => {
    const root = await bootApp();
    root.querySelector<HTMLElement>('[data-action="open-skill"]')!.click();
    const fileInput = document.querySelector<HTMLInputElement>("#skill-file")!;
    const file = new File(["# My skill\nUse a checklist."], "SKILL.md", { type: "text/markdown" });
    Object.defineProperty(fileInput, "files", { configurable: true, value: [file] });
    fileInput.dispatchEvent(new Event("change", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(50);
    expect(document.querySelector<HTMLTextAreaElement>("#editor-input")!.value).toBe("# My skill\nUse a checklist.");
    document.querySelector<HTMLElement>('[data-action="save-editor"]')!.click();
    await vi.advanceTimersByTimeAsync(20);
    root.querySelector<HTMLElement>('[data-action="open-skill"]')!.click();
    expect(document.querySelector<HTMLTextAreaElement>("#editor-input")!.value).toContain("Use a checklist.");
  });

  it("loads an already installed model without offering a download", async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const root = document.getElementById("app")!;
    const bridge = createBridge();
    const bootstrap = bridge.bootstrap.bind(bridge);
    bridge.bootstrap = async () => { const info = await bootstrap(); info.model.status = "loading"; return info; };
    Object.defineProperty(bridge, "demo", { value: false });
    const download = vi.spyOn(bridge, "downloadModel");
    mount(root, bridge);
    await vi.advanceTimersByTimeAsync(20);
    expect(root.textContent).toContain("Loading");
    expect(root.textContent).not.toContain("Download model");
    expect(download).not.toHaveBeenCalled();
  });

  it("shows the message and thinking immediately", async () => {
    const root = await bootApp();
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Hello now";
    root.querySelector<HTMLElement>('[data-action="send"]')?.click();
    expect(root.textContent).toContain("Hello now");
    expect(root.textContent).toContain("Thinking");
    expect(root.querySelector('[data-action="stop"]')).not.toBeNull();
  });

  it("keeps the outgoing message visible while the model is still pending", async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const root = document.getElementById("app") as HTMLElement;
    const bridge = createBridge();
    bridge.sendMessage = () => new Promise<void>(() => {});
    mount(root, bridge);
    await vi.advanceTimersByTimeAsync(20);
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Still waiting";
    root.querySelector<HTMLElement>('[data-action="send"]')!.click();
    expect(root.textContent).toContain("Still waiting");
    expect(root.textContent).toContain("Thinking");
    expect(root.querySelector('[data-action="stop"]')).not.toBeNull();
  });

  it("restores a draft when sending fails", async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const root = document.getElementById("app") as HTMLElement;
    const bridge = createBridge();
    bridge.sendMessage = async () => { throw new Error("Not ready"); };
    mount(root, bridge);
    await vi.advanceTimersByTimeAsync(20);
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Keep this draft";
    root.querySelector<HTMLElement>('[data-action="send"]')!.click();
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelector<HTMLTextAreaElement>("#input")!.value).toBe("Keep this draft");
    expect(root.textContent).toContain("Not ready");
  });

  it("waits for transcript persistence before allowing another turn", async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const root = document.getElementById("app")!;
    const bridge = createBridge();
    let emit!: (event: AgentEvent) => void;
    bridge.onEvent = async handler => { emit = handler; return () => {}; };
    let finish!: () => void;
    const send = vi.fn(() => new Promise<void>(resolve => { finish = resolve; }));
    bridge.sendMessage = send;
    mount(root, bridge);
    await vi.advanceTimersByTimeAsync(20);
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "First";
    root.querySelector<HTMLElement>('[data-action="send"]')!.click();
    emit({ type: "turn_started", turn_id: "t", session_id: "demo-session" });
    emit({ type: "turn_finished", turn_id: "t", stop: "answered", rounds: 1, prompt_tokens: 1, output_tokens: 1, tokens_per_second: 1 });
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Second";
    root.querySelector<HTMLTextAreaElement>("#input")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(send).toHaveBeenCalledTimes(1);
    expect(root.querySelector('[data-action="stop"]')).not.toBeNull();
    finish();
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelector('[data-action="stop"]')).toBeNull();
  });

  it("keeps sent messages and replies when reopening a chat", async () => {
    const root = await bootApp();
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "A visible question";
    root.querySelector<HTMLElement>('[data-action="send"]')!.click();
    await vi.advanceTimersByTimeAsync(5000);
    expect(root.querySelectorAll(".msg--user")).toHaveLength(2);
    expect(root.textContent).toContain("A visible question");
    expect(root.textContent).toContain("Demo response. No model is running.");
    root.querySelector<HTMLElement>('[data-action="new-session"]')!.click();
    await vi.advanceTimersByTimeAsync(20);
    const oldChat = root.querySelector<HTMLElement>('[data-action="open-session"][data-id="demo-session"]');
    oldChat!.click();
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelectorAll(".msg--user")).toHaveLength(2);
    expect(root.textContent).toContain("A visible question");
    expect(root.textContent).toContain("Demo response. No model is running.");
  });

  it("retains a sent message if the turn fails after it was saved", async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const root = document.getElementById("app")!;
    const bridge = createBridge();
    const load = bridge.loadSession.bind(bridge);
    let persisted = false;
    bridge.loadSession = async id => {
      const session = await load(id);
      if (persisted) session.messages.push({ id: "saved-user", role: "user", content: "Saved before error", attachments: [], tool_calls: [], created_at: Date.now() });
      return session;
    };
    bridge.sendMessage = async () => { persisted = true; throw new Error("Generation failed"); };
    mount(root, bridge);
    await vi.advanceTimersByTimeAsync(20);
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Saved before error";
    root.querySelector<HTMLElement>('[data-action="send"]')!.click();
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelectorAll(".msg--user")).toHaveLength(2);
    expect(root.querySelectorAll(".msg--user")[1]?.textContent).toContain("Saved before error");
    expect(root.querySelector<HTMLTextAreaElement>("#input")!.value).toBe("");
    expect(root.textContent).toContain("Generation failed");
  });

  it("keeps the visible message when a successful turn cannot be reloaded", async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const root = document.getElementById("app")!;
    const bridge = createBridge();
    const load = bridge.loadSession.bind(bridge);
    let sent = false;
    bridge.sendMessage = async () => { sent = true; };
    bridge.loadSession = async id => { if (sent) throw new Error("Reload failed"); return load(id); };
    mount(root, bridge);
    await vi.advanceTimersByTimeAsync(20);
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Do not hide this";
    root.querySelector<HTMLElement>('[data-action="send"]')!.click();
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelector(".messages")?.textContent).toContain("Do not hide this");
    expect(root.querySelector<HTMLTextAreaElement>("#input")!.value).toBe("");
    expect(root.textContent).toContain("Reload failed");
  });

  it("anchors a new turn and preserves manual scroll through tool updates", async () => {
    // JSDOM does not lay out elements; model a scrollable viewport and a
    // latest-turn anchor, including growth when a tool card appears.
    const height = vi.spyOn(Element.prototype, "clientHeight", "get").mockImplementation(function (this: Element) {
      return this.classList.contains("messages") ? 400 : 0;
    });
    const scrollHeight = vi.spyOn(Element.prototype, "scrollHeight", "get").mockImplementation(function (this: Element) {
      if (!this.classList.contains("messages")) return 0;
      if (this.querySelector(".stream-text")?.textContent) return 1600;
      return this.querySelector(".tool-card") ? 1400 : 1200;
    });
    const rect = vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      const messages = this.closest<HTMLElement>(".messages");
      const top = this.hasAttribute("data-latest-turn") ? 800 - (messages?.scrollTop ?? 0) : 0;
      return { top, bottom: top, left: 0, right: 0, width: 0, height: 0, x: 0, y: top, toJSON() {} } as DOMRect;
    });
    try {
      document.body.innerHTML = '<div id="app"></div>';
      const root = document.getElementById("app")!;
      const bridge = createBridge();
      let emit!: (event: AgentEvent) => void;
      bridge.onEvent = async handler => { emit = handler; return () => {}; };
      bridge.sendMessage = () => new Promise<void>(() => {});
      mount(root, bridge);
      await vi.advanceTimersByTimeAsync(20);
      root.querySelector<HTMLTextAreaElement>("#input")!.value = "New turn";
      root.querySelector<HTMLElement>('[data-action="send"]')!.click();
      expect(root.querySelector<HTMLElement>(".messages")!.scrollTop).toBe(800);
      expect(root.querySelector("[data-latest-turn] .msg--user")?.textContent).toContain("New turn");
      emit({ type: "turn_started", turn_id: "t", session_id: "demo-session" });
      let messages = root.querySelector<HTMLElement>(".messages")!;
      messages.scrollTop = 300; // user is reading earlier messages
      emit({ type: "tool_call_started", turn_id: "t", call_id: "c", name: "open_page", preview: "", risk: "" });
      messages = root.querySelector<HTMLElement>(".messages")!;
      expect(messages.scrollTop).toBe(300);
      emit({ type: "tool_call_finished", turn_id: "t", call_id: "c", name: "open_page", ok: true, summary: "Done", detail: "Result" });
      messages = root.querySelector<HTMLElement>(".messages")!;
      expect(messages.scrollTop).toBe(300);
      // Reasoning can start after the tool has already created the live block.
      emit({ type: "thinking_delta", turn_id: "t", text: "Now checking" });
      messages = root.querySelector<HTMLElement>(".messages")!;
      expect(messages.querySelector(".stream-thinking")?.textContent).toBe("Now checking");
      messages.scrollTop = 1000; // at the bottom before the content grows
      emit({ type: "text_delta", turn_id: "t", text: "An answer" });
      expect(messages.scrollTop).toBe(1600);
      messages.scrollTop = 300;
      const details = messages.querySelector<HTMLDetailsElement>(".tool-card details")!;
      details.open = true;
      emit({ type: "notice", message: "Status" });
      expect(root.querySelector<HTMLDetailsElement>(".tool-card details")!.open).toBe(true);
      expect(root.querySelector<HTMLElement>(".messages")!.scrollTop).toBe(300);
    } finally {
      height.mockRestore(); scrollHeight.mockRestore(); rect.mockRestore();
    }
  });

  it("keeps Stop available after a mid-turn error until the command settles", async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const root = document.getElementById("app")!;
    const bridge = createBridge();
    let emit!: (event: AgentEvent) => void;
    let finish!: () => void;
    bridge.onEvent = async handler => { emit = handler; return () => {}; };
    bridge.sendMessage = () => new Promise<void>(resolve => { finish = resolve; });
    mount(root, bridge);
    await vi.advanceTimersByTimeAsync(20);
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Keep working";
    root.querySelector<HTMLElement>('[data-action="send"]')!.click();
    emit({ type: "turn_started", turn_id: "t", session_id: "demo-session" });
    emit({ type: "error", turn_id: "t", message: "Temporary failure" });
    expect(root.querySelector('[data-action="stop"]')).not.toBeNull();
    expect(root.querySelector('[data-action="send"]')).toBeNull();
    emit({ type: "turn_finished", turn_id: "t", stop: "failed", rounds: 0, prompt_tokens: 0, output_tokens: 0, tokens_per_second: 0 });
    expect(root.querySelector('[data-action="stop"]')).not.toBeNull();
    finish();
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelector('[data-action="send"]')).not.toBeNull();
  });

  it("does not unlock an active turn when a send command is rejected", async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const root = document.getElementById("app")!;
    const bridge = createBridge();
    let emit!: (event: AgentEvent) => void;
    bridge.onEvent = async handler => { emit = handler; return () => {}; };
    bridge.sendMessage = async () => {
      emit({ type: "turn_started", turn_id: "t", session_id: "demo-session" });
      throw new Error("Cordial is still answering the previous message.");
    };
    mount(root, bridge);
    await vi.advanceTimersByTimeAsync(20);
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Duplicate";
    root.querySelector<HTMLElement>('[data-action="send"]')!.click();
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelector('[data-action="stop"]')).not.toBeNull();
    expect(root.querySelector('[data-action="send"]')).toBeNull();
    emit({ type: "turn_finished", turn_id: "t", stop: "answered", rounds: 1, prompt_tokens: 1, output_tokens: 1, tokens_per_second: 1 });
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelector('[data-action="send"]')).not.toBeNull();
  });

  it("restores a busy harness after reload and unlocks when it finishes", async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const root = document.getElementById("app")!;
    const bridge = createBridge();
    const bootstrap = bridge.bootstrap.bind(bridge);
    const load = bridge.loadSession.bind(bridge);
    let emit!: (event: AgentEvent) => void;
    bridge.onEvent = async handler => { emit = handler; return () => {}; };
    bridge.bootstrap = async () => ({ ...(await bootstrap()), busy: true });
    bridge.loadSession = async id => {
      const session = await load(id);
      session.messages.push({ id: "recovered", role: "assistant", content: "Recovered answer", tool_calls: [], attachments: [], created_at: Date.now() });
      return session;
    };
    mount(root, bridge);
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelector('[data-action="stop"]')).not.toBeNull();
    emit({ type: "turn_finished", turn_id: "recovered-turn", stop: "answered", rounds: 1, prompt_tokens: 1, output_tokens: 1, tokens_per_second: 1 });
    await vi.advanceTimersByTimeAsync(20);
    expect(root.querySelector('[data-action="send"]')).not.toBeNull();
    expect(root.textContent).toContain("Recovered answer");
  });

  it("creates a new chat from the sidebar", async () => {
    const root = await bootApp();
    root.querySelector<HTMLElement>('[data-action="new-session"]')?.click();
    await vi.advanceTimersByTimeAsync(20);

    expect(root.querySelector(".topbar__title")?.textContent).toBe("New chat");
    expect(root.textContent).toContain("What should we look into?");
  });

  it("sends a suggested prompt with one click", async () => {
    const root = await bootApp();
    root.querySelector<HTMLElement>('[data-action="new-session"]')?.click();
    await vi.advanceTimersByTimeAsync(20);

    root.querySelector<HTMLElement>('[data-action="suggestion"]')?.click();
    await vi.advanceTimersByTimeAsync(5000);

    expect(root.querySelectorAll(".msg").length).toBeGreaterThanOrEqual(2);
  });
});
