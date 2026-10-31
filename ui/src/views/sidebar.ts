import { contextLabel, escapeHtml, relativeDate } from "../format";
import { icons, logoMark } from "../icons";
import type { AppState } from "../store";

export function sidebarHtml(state: AppState): string {
  const rows = state.sessions
    .map((session) => {
      const active = state.session?.id === session.id;
      return `<li class="session-item">
        <button class="session-row" data-action="open-session" data-id="${escapeHtml(session.id)}" aria-current="${active}">
          <span class="session-row__title">${escapeHtml(session.title)}</span>
          <span class="mono" style="font-size:10px;color:var(--muted)">${relativeDate(session.updated_at)}</span>
        </button>
        <button class="session-row__delete" data-action="delete-session" data-id="${escapeHtml(
          session.id,
        )}" aria-label="Delete ${escapeHtml(session.title)}">${icons.trash(14)}</button>
      </li>`;
    })
    .join("");

  const chips = `${contextLabel(state.model.context_tokens || state.settings.context_tokens)} · ${escapeHtml(
    state.model.backend.toUpperCase(),
  )}`;

  return `<aside class="sidebar">
    <div class="brand">
      <span class="brand__mark">${logoMark(28)}</span>
      <div>
        <div class="brand__name">Cordial</div>
        <span class="brand__sub">Local agent · LFM3</span>
      </div>
    </div>
    <div class="sidebar__actions">
      <button class="new-chat" data-action="new-session">${icons.plus(14)} New chat</button>
    </div>
    <div class="sidebar__label">Chats</div>
    <ul class="session-list">${rows || `<li class="sidebar__empty">No chats yet.</li>`}</ul>
    <div class="sidebar__footer">
      <button class="model-chip" data-action="open-settings" aria-label="Model and settings">
        <span>
          <strong>Q4_K_M · LFM3-VL-2B</strong>
          <span class="mono">${chips}</span>
        </span>
        <span style="display:flex;align-items:center;gap:6px">
          <span class="model-chip__dot" data-state="${state.model.status}" aria-hidden="true"></span>
          ${icons.gear(16)}
        </span>
      </button>
    </div>
  </aside>`;
}
