import { escapeHtml, excerpt, relativeDate, toolFamily, toolLabel } from "../format";
import { icons, logoMark } from "../icons";
import { renderMarkdown } from "../markdown";
import type { AppState, ToolView } from "../store";
import type { Message, PlanStep } from "../types";

const STATUS_LABEL: Record<ToolView["status"], string> = {
  running: "Running",
  awaiting: "Waiting for you",
  done: "Done",
  denied: "Declined",
  interrupted: "Interrupted",
};

export function toolCard(tool: ToolView): string {
  const family = toolFamily(tool.name);
  const detail = tool.detail
    ? `<details data-detail-key="tool-${escapeHtml(tool.id)}"><summary>Result</summary><pre class="tool-card__detail">${escapeHtml(tool.detail)}</pre></details>`
    : "";
  const thumbnail = tool.thumbnail
    ? `<img class="tool-card__shot" src="${escapeHtml(tool.thumbnail)}" alt="Screenshot the agent is looking at" />`
    : "";
  const summary = tool.summary
    ? `<div class="tool-card__summary">${escapeHtml(tool.summary)}</div>`
    : tool.preview
      ? `<div class="tool-card__summary">${escapeHtml(excerpt(tool.preview, 140))}</div>`
      : "";

  return `<div class="tool-card" data-family="${family}" data-status="${tool.status}">
    <div class="tool-card__head">
      <span class="tool-card__icon" aria-hidden="true">${toolIcon(family)}</span>
      <span class="tool-card__name">${escapeHtml(toolLabel(tool.name))}</span>
      <span class="tool-card__status">${STATUS_LABEL[tool.status]}</span>
    </div>
    ${summary}
    ${thumbnail}
    ${detail}
  </div>`;
}

function toolIcon(family: string): string {
  switch (family) {
    case "search":
      return icons.search(16);
    case "browser":
      return icons.globe(16);
    case "write":
      return icons.pen(16);
    default:
      return icons.file(16);
  }
}

function thinkingBlock(text: string, open: boolean, id: string): string {
  if (!text.trim()) return "";
  return `<details class="thinking" data-detail-key="thinking-${escapeHtml(id)}"${open ? " open" : ""}>
    <summary>${icons.chevron(12)} Reasoning</summary>
    <div class="thinking__body">${escapeHtml(text.trim())}</div>
  </details>`;
}

function attachmentsHtml(message: Message): string {
  if (message.attachments.length === 0) return "";
  const chips = message.attachments
    .map(
      (attachment) =>
        `<span class="chip">${attachment.kind === "image" ? icons.file(10) : icons.file(10)}<span class="chip__name">${escapeHtml(
          attachment.name,
        )}</span></span>`,
    )
    .join("");
  return `<div class="attachments">${chips}</div>`;
}

function userBlock(message: Message): string {
  return `<article class="msg msg--user">
    ${attachmentsHtml(message)}
    <div class="bubble-user">${escapeHtml(message.content)}</div>
    <div class="topbar__meta">${relativeDate(message.created_at)}</div>
  </article>`;
}

function assistantBlock(message: Message, tools: ToolView[]): string {
  const cards = tools.map((tool) => toolCard(tool)).join("");
  const body = message.content.trim()
    ? `<div class="prose">${renderMarkdown(message.content)}</div>`
    : "";
  const copy = message.content.trim()
    ? `<button class="msg__action" data-action="copy" data-id="${escapeHtml(message.id)}">Copy</button>`
    : "";
  return `<article class="msg msg--assistant">
    <div class="msg__role">${logoMark(14, "msg__role-mark")} Cordial ${copy}</div>
    ${thinkingBlock(message.thinking ?? "", false, message.id)}
    ${body}
    ${cards}
  </article>`;
}

/** Tool cards for an assistant turn, paired with their result messages. */
function toolsForCall(state: AppState, message: Message): ToolView[] {
  if (!state.session || message.tool_calls.length === 0) return [];
  return message.tool_calls.map((call) => {
    const result = state.session?.messages.find(
      (candidate) => candidate.role === "tool" && candidate.tool_call_id === call.id,
    );
    const denied = result?.content.startsWith("The user declined") ?? false;
    return {
      id: call.id,
      name: call.name,
      preview: call.arguments.map((argument) => `${argument.name}=${argument.value}`).join(", "),
      risk: "",
      status: result ? (denied ? "denied" : "done") : state.busy ? "running" : "interrupted",
      summary: result ? excerpt(result.content, 160) : "",
      detail: result ? result.content : "",
      thumbnail: null,
    };
  });
}

/** Hide in-progress protocol until it can be classified as a final answer. */
export function visibleStreamText(text: string): string {
  const trimmed = text.trimStart();
  if (trimmed.startsWith("{") || trimmed.startsWith("<")) return "";
  const toolStart = text.indexOf("<tool_call>");
  return toolStart < 0 ? text : text.slice(0, toolStart);
}

export function chatHtml(state: AppState): string {
  const session = state.session;
  const messages = session?.messages ?? [];
  const blocks: string[] = [];
  let latestUserIndex = -1;

  for (const message of messages) {
    if (message.role === "tool") continue;
    if (message.role === "user") {
      latestUserIndex = blocks.length;
      blocks.push(userBlock(message));
    } else {
      blocks.push(assistantBlock(message, toolsForCall(state, message)));
    }
  }

  if (state.turn) {
    const live = state.turn;
    const hasContent =
      live.text.trim().length > 0 || live.thinking.trim().length > 0 || live.tools.length > 0;
    if (hasContent) {
      blocks.push(`<article class="msg msg--assistant" data-streaming="true">
        <div class="msg__role">${logoMark(14, "msg__role-mark")} Cordial</div>
        ${live.thinking.trim() ? `<details class="thinking" data-detail-key="live-thinking" open><summary>${icons.chevron(12)} Reasoning</summary><div class="thinking__body stream-thinking">${escapeHtml(live.thinking)}</div></details>` : ""}
        <div class="prose stream-text">${renderMarkdown(visibleStreamText(live.text))}</div>
        ${live.tools.map((tool) => toolCard(tool)).join("")}
      </article>`);
    }
  }

  if (state.busy && (!state.turn || (!state.turn.text.trim() && !state.turn.thinking.trim() && state.turn.tools.length === 0))) {
    blocks.push(`<article class="msg msg--assistant"><div class="msg__role">${logoMark(14, "msg__role-mark")} Cordial</div><div class="prose"><p class="topbar__meta" role="status">Thinking…</p></div></article>`);
  }

  if (state.approval) {
    blocks.push(approvalHtml(state.approval, state.settings.permission_mode));
  }

  if (blocks.length === 0) {
    blocks.push(emptyState());
  }

  if (state.notice) {
    blocks.push(
      `<div class="tool-card" data-family="files" role="status"><div class="tool-card__head">${icons.check(14)}<span class="tool-card__name">Note</span></div><div class="tool-card__summary">${escapeHtml(
        state.notice,
      )}</div></div>`,
    );
  }

  if (state.error) {
    blocks.push(
      `<div class="tool-card" data-family="write" role="alert"><div class="tool-card__head">${icons.close(14)}<span class="tool-card__name">Error</span></div><div class="tool-card__summary">${escapeHtml(
        state.error,
      )}</div></div>`,
    );
  }

  const plan = session?.plan?.length ? planCard(session.plan) : "";
  const content = latestUserIndex < 0
    ? plan + blocks.join("")
    : `${blocks.slice(0, latestUserIndex).join("")}<div class="messages__turn" data-latest-turn>${plan}${blocks.slice(latestUserIndex).join("")}</div>`;
  return `<div class="messages" id="messages" tabindex="0" aria-label="Conversation"><div class="messages__inner">${content}</div></div>`;
}

function planCard(steps: PlanStep[]): string {
  const done = steps.filter((step) => step.status === "done" || step.status === "blocked").length;
  const rows = steps.map((step) => `<li class="plan__step" data-status="${step.status}">
    <span class="plan__mark" aria-hidden="true">${step.status === "done" ? "✓" : step.status === "blocked" ? "!" : step.status === "running" ? "→" : "○"}</span>
    <span class="plan__title">${escapeHtml(step.title)}${step.note ? `<span class="plan__note">${escapeHtml(step.note)}</span>` : ""}</span>
  </li>`).join("");
  return `<section class="plan" aria-label="Task checklist"><div class="plan__head"><span>Task checklist</span><span>${done}/${steps.length}</span></div><ol class="plan__steps">${rows}</ol></section>`;
}

function approvalHtml(tool: ToolView, mode: AppState["settings"]["permission_mode"]): string {
  const scope = mode === "auto"
    ? "Allowing grants this tool for the current conversation, including later actions."
    : "Allowing applies only to this action.";
  return `<div class="approval" role="group" aria-label="Tool approval">
    <div class="approval__text">
      Allow <strong>${escapeHtml(toolLabel(tool.name))}</strong>?
      <span class="approval__risk">${escapeHtml(tool.risk || "This changes local files.")} · ${escapeHtml(
        excerpt(tool.preview, 120),
      )}<br>${escapeHtml(scope)}</span>
    </div>
    <div class="approval__actions">
      <button class="outline-button" data-action="approve" data-call="${escapeHtml(tool.id)}" data-allow="false">Deny</button>
      <button class="primary-button" data-action="approve" data-call="${escapeHtml(tool.id)}" data-allow="true">Allow</button>
    </div>
  </div>`;
}

function emptyState(): string {
  return `<div class="empty">
    <h2>What should we look into?</h2>
    <div class="suggestions">
      <button class="suggestion" data-action="suggestion" data-text="What is the weather in Sydney this afternoon?">Weather in Sydney this afternoon</button>
      <button class="suggestion" data-action="suggestion" data-text="Summarise the latest release notes for llama.cpp">Latest llama.cpp release notes</button>
      <button class="suggestion" data-action="suggestion" data-text="List the files in my workspace and explain what is there">Look at my workspace files</button>
    </div>
  </div>`;
}
