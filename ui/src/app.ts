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
import { chatHtml, visibleStreamText } from "./views/chat";
import { composerHtml } from "./views/composer";
import { downloadHtml } from "./views/download";
import { sidebarHtml } from "./views/sidebar";

const STREAM_EVENTS = new Set<AgentEvent["type"]>(["thinking_delta", "text_delta"]);
const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "webp", "gif", "bmp"]);

export function mount(root: HTMLElement, bridge: Bridge): void {
  let state = initialState();
  let attachments: Attachment[] = [];
  let rendered = false;
  let sending = false;
  let preparingPdf = false;
  let turnEventDuringBoot = false;

  const dialogHost = document.createElement("div");
  document.body.append(dialogHost);

  // ---------------------------------------------------------------- render

  function topbarHtml(): string {
    const title = state.session?.title ?? "New chat";
    return `<header class="topbar"><h1 class="topbar__title">${escapeHtml(title)}</h1>${bridge.demo ? '<span class="topbar__meta">Demo</span>' : ""}</header>`;
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
      </main>
      ${composerHtml(state, attachments)}
    </div>`;
  }

  // Keep the latest user turn tall enough to place its start at the top of
  // the viewport, even before the assistant has produced a long response.
  function sizeLatestTurn(messages: HTMLElement): void {
    const turn = messages.querySelector<HTMLElement>("[data-latest-turn]");
    if (!turn) return;
    const style = getComputedStyle(messages);
    const padding = (parseFloat(style.paddingTop) || 0) + (parseFloat(style.paddingBottom) || 0);
    turn.style.minHeight = `${Math.max(0, messages.clientHeight - padding)}px`;
  }

  function scrollToLatest(messages: HTMLElement): void {
    const turn = messages.querySelector<HTMLElement>("[data-latest-turn]");
    if (!turn) { messages.scrollTop = messages.scrollHeight; return; }
    const padding = parseFloat(getComputedStyle(messages).paddingTop) || 0;
    messages.scrollTop += turn.getBoundingClientRect().top - messages.getBoundingClientRect().top - padding;
  }

  function render(position: "preserve" | "latest" = "preserve"): void {
    const messages = root.querySelector<HTMLElement>(".messages");
    const atBottom = messages ? messages.scrollHeight - messages.scrollTop - messages.clientHeight < 80 : true;
    const scrollTop = messages?.scrollTop ?? 0;
    const openDetails = new Map<string, boolean>();
    messages?.querySelectorAll<HTMLDetailsElement>("details[data-detail-key]").forEach((detail) => {
      if (detail.dataset.detailKey) openDetails.set(detail.dataset.detailKey, detail.open);
    });

    const input = root.querySelector<HTMLTextAreaElement>("#input");
    const draft = input?.value ?? "";
    const hadFocus = input ? document.activeElement === input : false;
    const caret = input?.selectionStart ?? draft.length;

    root.innerHTML = shellHtml();
    rendered = true;

    const nextInput = root.querySelector<HTMLTextAreaElement>("#input");
    if (nextInput) {
      nextInput.value = draft;
      autosize(nextInput);
      if (hadFocus) {
        nextInput.focus({ preventScroll: true });
        nextInput.setSelectionRange(caret, caret);
      }
    }
    const nextMessages = root.querySelector<HTMLElement>(".messages");
    if (nextMessages) {
      nextMessages.querySelectorAll<HTMLDetailsElement>("details[data-detail-key]").forEach((detail) => {
        const key = detail.dataset.detailKey;
        if (key && openDetails.has(key)) detail.open = openDetails.get(key)!;
      });
      sizeLatestTurn(nextMessages);
      if (position === "latest") scrollToLatest(nextMessages);
      else nextMessages.scrollTop = atBottom ? nextMessages.scrollHeight : scrollTop;
    }
  }

  /** Cheap update for streamed tokens: patches only the live bubble. */
  function patchStream(): void {
    if (!rendered) {
      render();
      return;
    }
    const block = root.querySelector<HTMLElement>('[data-streaming="true"]');
    if (!block || (state.turn?.thinking.trim() && !block.querySelector(".stream-thinking"))) {
      render();
      return;
    }
    const messages = root.querySelector<HTMLElement>(".messages");
    const atBottom = messages ? messages.scrollHeight - messages.scrollTop - messages.clientHeight < 80 : false;
    const text = block.querySelector<HTMLElement>(".stream-text");
    if (text && state.turn) text.innerHTML = renderMarkdown(visibleStreamText(state.turn.text));
    const thinking = block.querySelector<HTMLElement>(".stream-thinking");
    if (thinking && state.turn) thinking.textContent = state.turn.thinking;
    if (messages && atBottom) messages.scrollTop = messages.scrollHeight;
  }

  window.addEventListener("resize", () => {
    const messages = root.querySelector<HTMLElement>(".messages");
    if (!messages) return;
    const atBottom = messages.scrollHeight - messages.scrollTop - messages.clientHeight < 80;
    sizeLatestTurn(messages);
    if (atBottom) messages.scrollTop = messages.scrollHeight;
  });

  function autosize(element: HTMLTextAreaElement): void {
    element.style.height = "auto";
    element.style.height = `${Math.min(200, element.scrollHeight)}px`;
  }

  /** Quiet announcement for screen readers. */
  function announce(text: string): void {
    let region = document.getElementById("announcer");
    if (!region) {
      region = document.createElement("div");
      region.id = "announcer";
      region.className = "sr-only";
      region.setAttribute("aria-live", "polite");
      document.body.append(region);
    }
    region.textContent = text;
  }

  // -------------------------------------------------------------- settings

  function openEditor(kind: "memory" | "skill"): void {
    const isMemory = kind === "memory";
    const title = isMemory ? "Memory" : "Skills";
    const hint = isMemory
      ? "Saved locally and used across chats."
      : "Reusable instructions. Permissions still apply.";
    dialogHost.innerHTML = `<dialog id="editor-dialog" aria-labelledby="editor-title">
      <div class="dialog__head"><div><h2 class="dialog__title" id="editor-title">${title}</h2><p class="dialog__intro">${hint}</p></div>
      <button class="close-button" data-action="close-editor" aria-label="Close">${"×"}</button></div>
      <label class="sr-only" for="editor-input">${title}</label>
      <textarea id="editor-input" class="input editor-input" maxlength="4000" rows="4" placeholder="${isMemory ? "What should Cordial remember?" : "Add instructions for a task"}">${escapeHtml(state.settings[kind] || "")}</textarea>
      ${isMemory ? "" : '<label class="outline-button skill-import">Import SKILL.md<input id="skill-file" type="file" accept=".md,.txt,text/markdown,text/plain" class="sr-only"></label>'}
      <p id="editor-error" role="alert"></p><div class="settings__actions"><button class="ghost-button" data-action="close-editor">Cancel</button><button class="primary-button" data-action="save-editor" data-kind="${kind}">Save</button></div>
    </dialog>`;
    const dialog = dialogHost.querySelector<HTMLDialogElement>("#editor-dialog");
    if (dialog && typeof dialog.showModal === "function") dialog.showModal();
    else dialog?.setAttribute("open", "");
  }

  function closeEditor(): void { dialogHost.innerHTML = ""; }

  // ---------------------------------------------------------------- events

  async function refreshSessions(): Promise<void> {
    try {
      state.sessions = sortSessions(await bridge.listSessions());
    } catch {
      // Listing is best effort; the chat keeps working.
    }
  }

  function handle(event: AgentEvent): void {
    if (event.type === "turn_started" || event.type === "turn_finished") turnEventDuringBoot = true;
    state = applyEvent(state, event);
    // The backend persists the transcript after emitting turn_finished. Do not
    // accept a second turn or switch chats until that command has settled.
    if (sending && (event.type === "turn_finished" || event.type === "error")) state.busy = true;

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
      void refreshSessions().then(async () => {
        if (event.type !== "turn_finished") return;
        // A reloaded window has no owning send command to reload the saved
        // transcript, so do it here when the recovered turn finishes.
        if (!sending && state.session) {
          try { state.session = await bridge.loadSession(state.session.id); }
          catch { /* Keep the visible transcript if reload fails. */ }
        }
        render();
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
    if (state.busy || sending || preparingPdf) return;
    sending = true;
    const pending = attachments.slice();
    const optimisticId = `pending-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    state.busy = true;
    state.error = null;
    state.notice = null;
    let sessionId: string | null = null;
    let priorUserCount = 0;
    let sent = false;
    try {
      if (!state.session) state.session = await bridge.createSession();
      sessionId = state.session.id;
      priorUserCount = state.session.messages.filter(m => m.role === "user").length;
      const message = { id: optimisticId, role: "user" as const, content: text, tool_calls: [], attachments: pending, created_at: Date.now() };
      state.session = { ...state.session, messages: [...state.session.messages, message] };
      clearComposer();
      render("latest");
      await bridge.sendMessage(sessionId, text, pending.map(({ kind, path, name, inline_data }) => ({ kind, path, name, inline_data })) );
      sent = true;
      // The durable transcript is authoritative; it replaces the optimistic copy.
      const saved = await bridge.loadSession(sessionId);
      if (state.session?.id === sessionId) state.session = saved;
      state.busy = false;
      state.turn = null;
      await refreshSessions();
      render();
    } catch (error) {
      // A failed model turn can still have saved the user message. Reconcile
      // before deciding whether to restore the draft and avoid hiding it.
      let savedUser = sent;
      let verified = false;
      if (sessionId && !sent) {
        try {
          const saved = await bridge.loadSession(sessionId);
          verified = true;
          savedUser = saved.messages.filter(m => m.role === "user").length > priorUserCount;
          if (savedUser && state.session?.id === sessionId) state.session = saved;
        } catch { /* Keep the visible message if persistence cannot be checked. */ }
      }
      const restoreDraft = !sent && (!sessionId || (verified && !savedUser));
      if (restoreDraft && state.session) state.session = { ...state.session, messages: state.session.messages.filter(m => m.id !== optimisticId) };
      state.error = error instanceof Error ? error.message : String(error);
      if (restoreDraft) attachments = pending;
      if (restoreDraft) {
        const input = root.querySelector<HTMLTextAreaElement>("#input");
        if (input) input.value = text;
      }
    } finally {
      sending = false;
      // A rejected duplicate send must not unlock the turn that is still
      // running. Only the owning command may clear an active turn.
      if (state.turn) state.busy = true;
      else state.busy = false;
      render();
    }
  }

  async function switchSession(id: string): Promise<void> {
    try {
      const session = await bridge.loadSession(id);
      if (state.session?.id !== session.id) attachments = [];
      state.session = session;
      state.turn = null;
      state.approval = null;
      state.notice = null;
      state.error = null;
      render("latest");
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

  async function persistSettings(next: typeof state.settings): Promise<boolean> {
    try {
      state.model = await bridge.saveSettings(next);
      state.settings = next;
      state.downloadError = null;
      render();
      return true;
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
      render();
      return false;
    }
  }

  async function chooseFolder(): Promise<void> {
    try {
      const folder = await bridge.pickFolder();
      if (!folder) return;
      const ok = await persistSettings({ ...state.settings, workspace_dir: folder });
      if (ok) {
        state.workspace = folder;
        announce("Working folder updated.");
        render();
      }
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
      render();
    }
  }

  async function resetFolder(): Promise<void> {
    const ok = await persistSettings({ ...state.settings, workspace_dir: "" });
    if (ok) {
      try {
        const info = await bridge.bootstrap();
        state.workspace = info.workspace_dir;
      } catch {
        state.workspace = "";
      }
      announce("Using the default working folder.");
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

  function attachPdf(): void {
    if (state.busy || preparingPdf) return;
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".pdf,application/pdf";
    input.addEventListener("change", () => {
      const file = input.files?.[0];
      if (!file || state.busy || preparingPdf) return;
      const targetSession = state.session?.id ?? null;
      void (async () => {
        preparingPdf = true;
        state.error = null;
        state.notice = "Preparing PDF pages locally…";
        render();
        try {
          const { MAX_PDF_BYTES, preparePdf } = await import("./pdf");
          if (file.size > MAX_PDF_BYTES) throw new Error("PDF must be at most 10 MB.");
          const pages = await preparePdf(new Uint8Array(await file.arrayBuffer()));
          if (state.busy || (state.session?.id ?? null) !== targetSession) {
            throw new Error("The conversation changed while the PDF was being prepared. Please attach it again.");
          }
          if (attachments.filter((item) => item.inline_data !== undefined).length + pages.length * 2 > 6) {
            throw new Error("At most three PDF pages can be attached to one message. No pages from this PDF were attached.");
          }
          const id = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
          const prepared: Attachment[] = [];
          pages.forEach((page, index) => {
            const label = `${file.name}, page ${index + 1}`;
            prepared.push({ kind: "image", path: `inline:${id}:${index}:image`, name: `${label} (image)`, inline_data: page.image });
            prepared.push({ kind: "text", path: `inline:${id}:${index}:text`, name: `${label} (text)`, inline_data: page.text || "[No extractable text on this page. Read the page image.]" });
          });
          attachments = [...attachments, ...prepared];
          state.notice = `Attached ${pages.length} PDF page${pages.length === 1 ? "" : "s"} as images and extracted text.`;
        } catch (error) {
          state.error = error instanceof Error ? error.message : String(error);
          state.notice = null;
        } finally {
          preparingPdf = false;
          render();
        }
      })();
    }, { once: true });
    input.click();
  }

  // ------------------------------------------------------------- listeners

  root.addEventListener("click", (event) => {
    const target = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-action]");
    if (!target) return;
    const action = target.dataset.action;
    if (state.busy && ["new-session", "open-session", "delete-session", "open-memory", "open-skill", "remove-model", "choose-folder", "reset-folder"].includes(action || "")) return;

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
            attachments = [];
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
      case "attach-pdf":
        attachPdf();
        break;
      case "toggle-web":
        if (state.busy) break;
        if (!state.settings.web_enabled && !window.confirm("Web and browser tools can access local and private network services. Enable web access?")) break;
        void persistSettings({ ...state.settings, web_enabled: !state.settings.web_enabled });
        break;
      case "remove-attachment": {
        const index = Number.parseInt(target.dataset.index ?? "", 10);
        if (Number.isFinite(index)) {
          const path = attachments[index]?.path ?? "";
          const inline = /^inline:([^:]+):/.exec(path);
          attachments = inline
            ? attachments.filter((item) => !item.path.startsWith(`inline:${inline[1]}:`))
            : withoutAttachment(attachments, index);
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
      case "remove-model":
        if (window.confirm("Remove the downloaded model? Your chats and workspace will stay.")) {
          void (async () => {
            try {
              await bridge.deleteModel();
              state.model = { ...state.model, status: "missing", missing_bytes: 1_303_000_000 };
              state.phase = "missing";
              state.downloadError = null;
            } catch (error) { state.error = error instanceof Error ? error.message : String(error); }
            render();
          })();
        }
        break;
      case "choose-folder":
        void chooseFolder();
        break;
      case "reset-folder":
        void resetFolder();
        break;
      case "open-memory":
        openEditor("memory");
        break;
      case "open-skill":
        openEditor("skill");
        break;
      case "copy": {
        const id = target.dataset.id;
        const message = state.session?.messages.find((candidate) => candidate.id === id);
        if (message) {
          void navigator.clipboard
            ?.writeText(message.content)
            .then(() => announce("Copied to the clipboard."))
            .catch(() => announce("Copy failed. Select the text instead."));
        }
        break;
      }
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

  root.addEventListener("change", (event) => {
    const select = event.target as HTMLSelectElement;
    if (select.id === "effort-select" && ["low", "medium", "high", "task"].includes(select.value)) {
      void persistSettings({ ...state.settings, effort: select.value as Effort });
    } else if (select.id === "permission-select" && ["auto", "manual"].includes(select.value)) {
      void persistSettings({ ...state.settings, permission_mode: select.value as PermissionMode });
    }
  });

  dialogHost.addEventListener("change", (event) => {
    const fileInput = event.target as HTMLInputElement;
    if (fileInput.id !== "skill-file" || !fileInput.files?.length) return;
    const file = fileInput.files[0];
    const message = dialogHost.querySelector<HTMLElement>("#editor-error");
    const input = dialogHost.querySelector<HTMLTextAreaElement>("#editor-input");
    if (file.size > 32_000) {
      if (message) message.textContent = "File is too large. Use a skill under 4,000 characters.";
      fileInput.value = "";
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      const content = typeof reader.result === "string" ? reader.result : "";
      if (Array.from(content).length > 4000 || content.length > 4000) {
        if (message) message.textContent = "Skill must be 4,000 characters or less.";
      } else if (input) {
        input.value = content;
        if (message) message.textContent = "";
      }
      fileInput.value = "";
    };
    reader.onerror = () => { if (message) message.textContent = "Could not read file."; fileInput.value = ""; };
    reader.readAsText(file);
  });

  dialogHost.addEventListener("click", (event) => {
    const target = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-action]");
    if (target?.dataset.action === "close-editor") closeEditor();
    if (target?.dataset.action === "save-editor") {
      const kind = target.dataset.kind;
      const input = dialogHost.querySelector<HTMLTextAreaElement>("#editor-input");
      if ((kind === "memory" || kind === "skill") && input) {
        const next = { ...state.settings, [kind]: input.value.trim() };
        void (async () => {
          if (await persistSettings(next)) closeEditor();
          else {
            const message = dialogHost.querySelector<HTMLElement>("#editor-error");
            if (message) message.textContent = state.error || "Could not save.";
          }
        })();
      }
    }
  });

  // ------------------------------------------------------------------ boot

  async function boot(): Promise<void> {
    render();
    try {
      await bridge.onEvent(handle);
      const data = await bridge.bootstrap();
      state.settings = data.settings;
      state.model = data.model;
      state.app = data.app;
      state.workspace = data.workspace_dir;
      state.sessions = sortSessions(data.sessions);
      // A window can reload while the native harness is still answering.
      // Live events are newer than this snapshot, so never overwrite them.
      if (data.busy && !turnEventDuringBoot) state.busy = true;
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

      if (data.model.status === "error") state.downloadError = state.error || "Model could not be loaded.";
      render("latest");

      // First run: fetch the default Q4_K_M weights and projector without
      // waiting for a click. The button stays as a retry path.
      const needsDownload = state.model.status === "missing";
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
