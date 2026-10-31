import { escapeHtml, excerpt, relativeDate, toolFamily, toolLabel } from "../format";
import { icons, logoMark } from "../icons";
import { renderMarkdown } from "../markdown";
import type { AppState, ToolView } from "../store";
import type { Message } from "../types";

const STATUS_LABEL: Record<ToolView["status"], string> = {
  running: "Running",
  awaiting: "Waiting for you",
  done: "Done",
  denied: "Declined",
};

export function toolCard(tool: ToolView): string {
  const family = toolFamily(tool.name);
  const detail = tool.detail
    ? `<details><summary>Result</summary><pre class="tool-card__detail">${escapeHtml(tool.detail)}</pre></details>`
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

function thinkingBlock(text: string, open: boolean): string {
  if (!text.trim()) return "";
  return `<details class="thinking"${open ? " open" : ""}>
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
  return `<article class="msg msg--assistant">
    <div class="msg__role">${logoMark(14, "msg__role-mark")} Cordial</div>
    ${thinkingBlock(message.thinking ?? "", false)}
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
      status: result ? (denied ? "denied" : "done") : "running",
      summary: result ? excerpt(result.content, 160) : "",
      detail: result ? result.content : "",
      thumbnail: null,
    };
  });
}

export function chatHtml(state: AppState): string {
  const session = state.session;
  const messages = session?.messages ?? [];
  const blocks: string[] = [];

  for (const message of messages) {
    if (message.role === "tool") continue;
    if (message.role === "user") {
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
        ${live.thinking.trim() ? `<details class="thinking" open><summary>${icons.chevron(12)} Reasoning</summary><div class="thinking__body stream-thinking">${escapeHtml(live.thinking)}</div></details>` : ""}
        <div class="prose stream-text">${renderMarkdown(live.text)}</div>
        ${live.tools.map((tool) => toolCard(tool)).join("")}
      </article>`);
    }
  }

  if (state.busy && !state.turn) {
    blocks.push(`<article class="msg msg--assistant"><div class="msg__role">${logoMark(14, "msg__role-mark")} Cordial</div><div class="prose"><p class="topbar__meta">Thinking…</p></div></article>`);
  }

  if (state.approval) {
    blocks.push(approvalHtml(state.approval));
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
      `<div class="tool-card" data-family="write" role="alert"><div class="tool-card__head">${icons.close(14)}<span class="tool-card__name">Something went wrong</span></div><div class="tool-card__summary">${escapeHtml(
        state.error,
      )}</div></div>`,
    );
  }

  return `<div class="messages" id="messages"><div class="messages__inner">${blocks.join("")}</div></div>`;
}

function approvalHtml(tool: ToolView): string {
  return `<div class="approval" role="group" aria-label="Tool approval">
    <div class="approval__text">
      Cordial wants to run <strong>${escapeHtml(toolLabel(tool.name))}</strong>.
      <span class="approval__risk">${escapeHtml(tool.risk || "This changes local files.")} · ${escapeHtml(
        excerpt(tool.preview, 120),
      )}</span>
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
    <p>Cordial runs LFM3-VL-2B on this machine. It can search the web, read pages, drive a browser and work in your workspace.</p>
    <div class="suggestions">
      <button class="suggestion" data-action="suggestion" data-text="What is the weather in Sydney this afternoon?">Weather in Sydney this afternoon</button>
      <button class="suggestion" data-action="suggestion" data-text="Summarise the latest release notes for llama.cpp">Latest llama.cpp release notes</button>
      <button class="suggestion" data-action="suggestion" data-text="List the files in my workspace and explain what is there">Look at my workspace files</button>
    </div>
  </div>`;
}
