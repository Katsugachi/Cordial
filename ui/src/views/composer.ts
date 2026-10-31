import { escapeHtml } from "../format";
import { icons } from "../icons";
import type { AppState } from "../store";
import type { Attachment } from "../types";

export function composerHtml(state: AppState, attachments: Attachment[]): string {
  const chips = attachments
    .map(
      (attachment, index) => `<span class="chip">
        ${icons.file(10)}
        <span class="chip__name">${escapeHtml(attachment.name)}</span>
        <button class="chip__remove" data-action="remove-attachment" data-index="${index}" aria-label="Remove ${escapeHtml(
          attachment.name,
        )}">${icons.close(10)}</button>
      </span>`,
    )
    .join("");

  const busy = state.busy;
  const sendButton = busy
    ? `<button class="icon-button" data-action="stop" aria-label="Stop answering" title="Stop">${icons.stop(18)}</button>`
    : `<button class="primary-button" data-action="send" aria-label="Send message">${icons.send(16)} Send</button>`;

  return `<section class="composer">
    <div class="composer__inner">
      ${chips ? `<div class="attachments">${chips}</div>` : ""}
      <div class="composer__box">
        <button class="icon-button" data-action="attach" aria-label="Attach an image or file" title="Attach">${icons.attach(
          18,
        )}</button>
        <textarea class="composer__input" id="input" rows="1" placeholder="Ask anything. Cordial checks the web before it answers." aria-label="Message"></textarea>
        ${sendButton}
      </div>
      <div class="hint">
        <span>Enter to send · Shift + Enter for a new line</span>
        <span class="mono">${state.model.vision ? "Vision on" : "Text only"} · ${state.settings.context_tokens / 1024}k context · ${
          state.settings.permission_mode === "auto" ? "Auto approvals" : "Manual approvals"
        }</span>
      </div>
    </div>
  </section>`;
}
