// Composition root: renders the shell, wires user actions to the bridge and
// folds harness events into view state.

import type { Bridge } from "./bridge";
import { escapeHtml } from "./format";
import { renderMarkdown } from "./markdown";
import {
  applyEvent,
  initialState,
  showsChat,
  sortSessions,
  withAttachment,
  withoutAttachment,
} from "./store";
import type { AgentEvent, Attachment, Effort, PermissionMode } from "./types";
import { EFFORTS, PERMISSION_MODES, effortLabel, permissionLabel } from "./types";
import { chatHtml } from "./views/chat";
import { composerHtml } from "./views/composer";
import { downloadHtml } from "./views/download";
import { readSettingsForm, settingsHtml } from "./views/settings";
import { sidebarHtml } from "./views/sidebar";

const STREAM_EVENTS = new Set<AgentEvent["type"]>(["thinking_delta", "text_delta"]);
const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "webp", "gif", "bmp"]);

export function mount(root: HTMLElement, bridge: Bridge): void {
  let state = initialState();
  let attachments: Attachment[] = [];
  let rendered = false;

  const dialogHost = document.createElement("div");
  document.body.append(dialogHost);

  // ---------------------------------------------------------------- render

  function topbarHtml(): string {
    const title = state.session?.title ?? "New chat";
    const meta = bridge.demo
      ? "Demo preview · no model is running"
      : state.phase === "ready"
        ? `${state.model.description || "LFM3-VL-2B"} · ${state.model.backend.toUpperCase()} · ${state.model.context_tokens / 1024}k context`
        : state.phase === "loading"
          ? "Loading the local model…"
          : "Model not downloaded";

    return `<header class="topbar">
      <div style="min-width:0">
        <h1 class="topbar__title">${escapeHtml(title)}</h1>
        <div class="topbar__meta">${escapeHtml(meta)}</div>
      </div>
      <div class="topbar__controls">
        ${segmented("Permissions", PERMISSION_MODES, state.settings.permission_mode, "set-permission", permissionLabel)}
        ${segmented("Effort", EFFORTS, state.settings.effort, "set-effort", effortLabel)}
        ${state.busy ? `<span class="chip" role="status">Working…</span>` : ""}
      </div>
    </header>`;
  }

  function segmented<T extends string>(
    label: string,
    values: readonly T[],
    current: T,
    action: string,
    render: (value: T) => string,
  ): string {
    const buttons = values
      .map(
        (value) =>
          `<button class="segmented__button" data-action="${action}" data-value="${value}" aria-pressed="${
            value === current
          }">${render(value)}</button>`,
      )
      .join("");
    return `<div class="segmented" role="group" aria-label="${label}"><span class="segmented__label">${label}</span>${buttons}</div>`;
  }

  function shellHtml(): string {
    if (!showsChat(state)) {
      return downloadHtml(state);
    }
    return `<div class="app">
      ${sidebarHtml(state)}
      <main class="main">
        ${topbarHtml()}
        ${chatHtml(state)}
        ${composerHtml(state, attachments)}
      </main>
    </div>`;
  }

  function render(): void {
    const messages = root.querySelector<HTMLElement>(".messages");
    const atBottom = messages ? messages.scrollHeight - messages.scrollTop - messages.clientHeight < 80 : true;
    const scrollTop = messages?.scrollTop ?? 0;

    const input = root.querySelector<HTMLTextAreaElement>("#input");
    const draft = input?.value ?? "";
    const hadFocus = input ? document.activeElement === input : false;
    const caret = input?.selectionStart ?? draft.length;

    root.innerHTML = shellHtml();
    rendered = true;

    const nextMessages = root.querySelector<HTMLElement>(".messages");
    if (nextMessages) {
      nextMessages.scrollTop = atBottom ? nextMessages.scrollHeight : scrollTop;
    }

    const nextInput = root.querySelector<HTMLTextAreaElement>("#input");
    if (nextInput) {
      nextInput.value = draft;
      autosize(nextInput);
      if (hadFocus) {
        nextInput.focus();
        nextInput.setSelectionRange(caret, caret);
      }
    }
  }

  /** Cheap update for streamed tokens: patches only the live bubble. */
  function patchStream(): void {
    if (!rendered) {
      render();
      return;
    }
    const block = root.querySelector<HTMLElement>('[data-streaming="true"]');
    if (!block) {
      render();
      return;
    }
    const text = block.querySelector<HTMLElement>(".stream-text");
    if (text && state.turn) {
      text.innerHTML = renderMarkdown(state.turn.text);
    }
    const thinking = block.querySelector<HTMLElement>(".stream-thinking");
    if (thinking && state.turn) {
      thinking.textContent = state.turn.thinking;
    }
    const messages = root.querySelector<HTMLElement>(".messages");
    if (messages) {
      const atBottom = messages.scrollHeight - messages.scrollTop - messages.clientHeight < 120;
      if (atBottom) messages.scrollTop = messages.scrollHeight;
    }
  }

  function autosize(element: HTMLTextAreaElement): void {
    element.style.height = "auto";
    element.style.height = `${Math.min(200, element.scrollHeight)}px`;
  }

  // -------------------------------------------------------------- settings

  function openSettings(): void {
    dialogHost.innerHTML = settingsHtml(state);
    const dialog = dialogHost.querySelector<HTMLDialogElement>("#settings-dialog");
    if (!dialog || dialog.open) return;
    // Older WebKitGTK builds ship <dialog> without showModal(); fall back to
    // the attribute so the panel still appears.
    if (typeof dialog.showModal === "function") {
      dialog.showModal();
    } else {
      dialog.setAttribute("open", "");
    }
  }

  function closeSettings(): void {
    dialogHost.innerHTML = "";
  }

  // ---------------------------------------------------------------- events

  async function refreshSessions(): Promise<void> {
    try {
      state.sessions = sortSessions(await bridge.listSessions());
    } catch {
      // Listing is best effort; the chat keeps working.
    }
  }

  function handle(event: AgentEvent): void {
    state = applyEvent(state, event);

    if (event.type === "error" && !showsChat(state) && !state.download) {
      state.downloadError = event.message;
      state.phase = "missing";
    }
    if (event.type === "model_ready") {
      state.phase = "ready";
      state.download = null;
    }

    if (STREAM_EVENTS.has(event.type)) {
      patchStream();
      return;
    }

    render();

    if (event.type === "turn_finished" || event.type === "assistant_message") {
      void refreshSessions().then(() => {
        if (event.type === "turn_finished") render();
      });
    }
  }

  // --------------------------------------------------------------- actions

  function draftText(): string {
    return root.querySelector<HTMLTextAreaElement>("#input")?.value.trim() ?? "";
  }

  function clearComposer(): void {
    const input = root.querySelector<HTMLTextAreaElement>("#input");
    if (input) {
      input.value = "";
      autosize(input);
    }
    attachments = [];
  }

  async function send(text: string): Promise<void> {
    if (state.busy) return;
    let session = state.session;
    try {
      if (!session) {
        session = await bridge.createSession();
        state.session = session;
        await refreshSessions();
      }
      const payload = attachments.map((attachment) => ({ kind: attachment.kind, path: attachment.path }));
      clearComposer();
      render();
      await bridge.sendMessage(session.id, text, payload);
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
      render();
    }
  }

  async function switchSession(id: string): Promise<void> {
    try {
      state.session = await bridge.loadSession(id);
      state.turn = null;
      state.approval = null;
      state.notice = null;
      state.error = null;
      render();
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
      render();
    }
  }

  async function newSession(): Promise<void> {
    try {
      state.session = await bridge.createSession();
      state.turn = null;
      state.approval = null;
      attachments = [];
      await refreshSessions();
      render();
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
      render();
    }
  }

  async function persistSettings(next: typeof state.settings): Promise<void> {
    try {
      state.model = await bridge.saveSettings(next);
      state.settings = next;
      state.downloadError = null;
      render();
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
      render();
    }
  }

  async function attachFiles(): Promise<void> {
    try {
      const paths = await bridge.pickFiles();
      for (const path of paths) {
        const name = path.split(/[\\/]/).pop() ?? path;
        const extension = name.split(".").pop()?.toLowerCase() ?? "";
        attachments = withAttachment(attachments, {
          kind: IMAGE_EXTENSIONS.has(extension) ? "image" : "text",
          name,
          path,
        });
      }
      render();
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
      render();
    }
  }

  // ------------------------------------------------------------- listeners

  root.addEventListener("click", (event) => {
    const target = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-action]");
    if (!target) return;
    const action = target.dataset.action;

    switch (action) {
      case "new-session":
        void newSession();
        break;
      case "open-session":
        if (target.dataset.id) void switchSession(target.dataset.id);
        break;
      case "delete-session": {
        const id = target.dataset.id;
        if (!id) break;
        void (async () => {
          await bridge.deleteSession(id);
          await refreshSessions();
          if (state.session?.id === id) {
            state.session = state.sessions[0] ? await bridge.loadSession(state.sessions[0].id) : await bridge.createSession();
          }
          render();
        })();
        break;
      }
      case "send": {
        const text = draftText();
        if (text) void send(text);
        break;
      }
      case "stop":
        void bridge.cancelTurn();
        break;
      case "approve":
        if (target.dataset.call) {
          void bridge.approveTool(target.dataset.call, target.dataset.allow === "true");
        }
        break;
      case "attach":
        void attachFiles();
        break;
      case "remove-attachment": {
        const index = Number.parseInt(target.dataset.index ?? "", 10);
        if (Number.isFinite(index)) {
          attachments = withoutAttachment(attachments, index);
          render();
        }
        break;
      }
      case "suggestion": {
        const text = target.dataset.text ?? "";
        const input = root.querySelector<HTMLTextAreaElement>("#input");
        if (input) input.value = text;
        void send(text);
        break;
      }
      case "set-effort":
        if (target.dataset.value) {
          void persistSettings({ ...state.settings, effort: target.dataset.value as Effort });
        }
        break;
      case "set-permission":
        if (target.dataset.value) {
          void persistSettings({ ...state.settings, permission_mode: target.dataset.value as PermissionMode });
        }
        break;
      case "open-settings":
        openSettings();
        break;
      case "download":
        state.downloadError = null;
        render();
        void bridge.downloadModel().catch((error: unknown) => {
          state.downloadError = error instanceof Error ? error.message : String(error);
          render();
        });
        break;
      default:
        break;
    }
  });

  root.addEventListener("keydown", (event) => {
    const input = event.target as HTMLTextAreaElement | null;
    if (input?.id !== "input") return;
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      const text = input.value.trim();
      if (text) void send(text);
    }
  });

  root.addEventListener("input", (event) => {
    const input = event.target as HTMLTextAreaElement | null;
    if (input?.id === "input") autosize(input);
  });

  dialogHost.addEventListener("click", (event) => {
    const target = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-action]");
    if (!target) return;
    switch (target.dataset.action) {
      case "close-settings":
        closeSettings();
        break;
      case "save-settings": {
        const next = readSettingsForm(dialogHost, state.settings);
        closeSettings();
        void persistSettings(next);
        break;
      }
      case "open-workspace":
        void bridge.openPath(state.workspace).catch(() => undefined);
        break;
      case "delete-model":
        void (async () => {
          await bridge.deleteModel().catch(() => undefined);
          state.model = { ...state.model, status: "missing", missing_bytes: 1_303_000_000, vision: false };
          state.phase = "missing";
          closeSettings();
          render();
        })();
        break;
      default:
        break;
    }
  });

  // ------------------------------------------------------------------ boot

  async function boot(): Promise<void> {
    render();
    try {
      const data = await bridge.bootstrap();
      state.settings = data.settings;
      state.model = data.model;
      state.app = data.app;
      state.workspace = data.workspace_dir;
      state.sessions = sortSessions(data.sessions);
      state.phase = bridge.demo
        ? "demo"
        : data.model.status === "ready"
          ? "ready"
          : data.model.status === "loading"
            ? "loading"
            : "missing";

      if (state.sessions.length > 0) {
        try {
          state.session = await bridge.loadSession(state.sessions[0].id);
        } catch {
          state.session = null;
        }
      }
      if (!state.session) {
        state.session = await bridge.createSession();
        state.sessions = sortSessions(await bridge.listSessions());
      }

      render();
      await bridge.onEvent(handle);

      // First run: fetch the default Q4_K_M weights and projector without
      // waiting for a click. The button stays as a retry path.
      const needsDownload = state.phase === "missing" || state.model.status === "text_only";
      if (needsDownload && !bridge.demo) {
        void bridge.downloadModel().catch((error: unknown) => {
          state.downloadError = error instanceof Error ? error.message : String(error);
          render();
        });
      }
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
      state.phase = "missing";
      render();
    }
  }

  void boot();
}
