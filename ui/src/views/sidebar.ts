import { escapeHtml, relativeDate } from "../format";
import { icons, logoMark } from "../icons";
import type { AppState } from "../store";

export function sidebarHtml(state: AppState): string {
  const rows = state.sessions.map((session) => {
    const active = state.session?.id === session.id;
    return `<li class="session-item">
      <button class="session-row" data-action="open-session" ${state.busy ? "disabled" : ""} data-id="${escapeHtml(session.id)}" aria-current="${active}">
        <span class="session-row__title">${escapeHtml(session.title)}</span>
        <span class="session-row__date">${relativeDate(session.updated_at)}</span>
      </button>
      <button class="session-row__delete" data-action="delete-session" ${state.busy ? "disabled" : ""} data-id="${escapeHtml(session.id)}" aria-label="Delete ${escapeHtml(session.title)}">${icons.trash(14)}</button>
    </li>`;
  }).join("");
  return `<aside class="sidebar">
    <div class="brand"><span class="brand__mark">${logoMark(28)}</span><div class="brand__name">Cordial</div></div>
    <div class="sidebar__actions"><button class="new-chat" data-action="new-session" ${state.busy ? "disabled" : ""}>${icons.plus(14)} New chat</button></div>
    <div class="sidebar__label">Chats</div>
    <ul class="session-list">${rows || `<li class="sidebar__empty">No chats yet.</li>`}</ul>
    <div class="sidebar__footer sidebar__tools">
      <button class="ghost-button" data-action="open-memory" ${state.busy ? "disabled" : ""}>Memory</button>
      <button class="ghost-button" data-action="open-skill" ${state.busy ? "disabled" : ""}>Skills</button>
    </div>
  </aside>`;
}
