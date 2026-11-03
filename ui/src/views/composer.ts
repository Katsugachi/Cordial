import { escapeHtml } from "../format";
import { icons } from "../icons";
import type { AppState } from "../store";
import type { Attachment } from "../types";

export function composerHtml(state: AppState, attachments: Attachment[]): string {
  const chips = attachments.map((attachment, index) => `<span class="chip">
    ${icons.file(10)} <span class="chip__name">${escapeHtml(attachment.name)}</span>
    <button class="chip__remove" data-action="remove-attachment" data-index="${index}" aria-label="Remove ${escapeHtml(attachment.name)}">${icons.close(10)}</button>
  </span>`).join("");
  const sendButton = state.busy
    ? `<button class="icon-button" data-action="stop" aria-label="Stop answering" title="Stop">${icons.stop(18)}</button>`
    : `<button class="primary-button" data-action="send" aria-label="Send message">${icons.send(16)} Send</button>`;
  return `<section class="composer"><div class="composer__inner">
    ${chips ? `<div class="attachments">${chips}</div>` : ""}
    <div class="composer__box">
      <textarea class="composer__input" id="input" rows="1" placeholder="Ask anything..." aria-label="Message"></textarea>
      <div class="composer__toolbar">
        <button class="icon-button" data-action="attach" aria-label="Attach an image or file" title="Attach">${icons.attach(18)}</button>
        <label class="sr-only" for="permission-select">Permissions</label>
        <select id="permission-select" class="composer__select" aria-label="Permissions" ${state.busy ? "disabled" : ""}>
          <option value="auto" ${state.settings.permission_mode === "auto" ? "selected" : ""}>Auto</option>
          <option value="manual" ${state.settings.permission_mode === "manual" ? "selected" : ""}>Manual</option>
        </select>
        <label class="sr-only" for="effort-select">Effort</label>
        <select id="effort-select" class="composer__select" aria-label="Effort" ${state.busy ? "disabled" : ""}>
          <option value="low" ${state.settings.effort === "low" ? "selected" : ""}>Low</option>
          <option value="medium" ${state.settings.effort === "medium" ? "selected" : ""}>Medium</option>
          <option value="high" ${state.settings.effort === "high" ? "selected" : ""}>High</option>
          <option value="task" ${state.settings.effort === "task" ? "selected" : ""}>Task</option>
        </select>
        ${sendButton}
      </div>
    </div>
  </div></section>`;
}
