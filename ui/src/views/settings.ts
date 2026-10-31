import { escapeHtml } from "../format";
import { icons } from "../icons";
import type { AppState } from "../store";
import type { Effort, PermissionMode, Settings } from "../types";
import { effortLabel, permissionLabel } from "../types";

export function settingsHtml(state: AppState): string {
  const settings = state.settings;
  const model = state.model;

  return `<dialog id="settings-dialog" aria-labelledby="settings-title">
    <div class="dialog__head">
      <div>
        <h2 class="dialog__title" id="settings-title">Settings</h2>
        <p class="dialog__intro">Cordial runs LFM3-VL-2B locally. These controls shape how it works and how much it is allowed to do.</p>
      </div>
      <button class="close-button" data-action="close-settings" aria-label="Close settings">${icons.close(18)}</button>
    </div>

    <div class="field-row">
      <div class="field">
        <label class="field__label" for="setting-effort">Effort</label>
        <select class="select" id="setting-effort">
          <option value="low"${settings.effort === "low" ? " selected" : ""}>Low — quick, few tool calls</option>
          <option value="medium"${settings.effort === "medium" ? " selected" : ""}>Medium — balanced</option>
          <option value="high"${settings.effort === "high" ? " selected" : ""}>High — thorough</option>
        </select>
        <div class="field__hint">${effortLabel(settings.effort)} caps how many tool rounds one answer may use.</div>
      </div>
      <div class="field">
        <label class="field__label" for="setting-permission">Permissions</label>
        <select class="select" id="setting-permission">
          <option value="auto"${settings.permission_mode === "auto" ? " selected" : ""}>Auto — ask only for writes</option>
          <option value="manual"${settings.permission_mode === "manual" ? " selected" : ""}>Manual — ask every time</option>
        </select>
        <div class="field__hint">${permissionLabel(settings.permission_mode)} mode is active.</div>
      </div>
    </div>

    <div class="field-row">
      <div class="field">
        <label class="field__label" for="setting-context">Context window</label>
        <input class="input input--numeric" id="setting-context" type="number" min="2048" max="131072" step="1024" value="${settings.context_tokens}" />
        <div class="field__hint">Tokens. LFM3-VL-2B supports 128k; larger values use more memory.</div>
      </div>
      <div class="field">
        <label class="field__label" for="setting-threads">CPU threads</label>
        <input class="input input--numeric" id="setting-threads" type="number" min="1" max="64" value="${settings.threads || 0}" />
        <div class="field__hint">0 lets Cordial pick (cores − 1).</div>
      </div>
    </div>

    <div class="field">
      <label class="field__label" for="setting-workspace">Workspace folder</label>
      <input class="input" id="setting-workspace" type="text" value="${escapeHtml(settings.workspace_dir)}" placeholder="${escapeHtml(
        state.workspace,
      )}" />
      <div class="field__hint">Cordial only reads and writes inside this folder.</div>
    </div>

    <div class="field">
      <label class="checkrow" for="setting-web">
        <input class="checkbox" id="setting-web" type="checkbox"${settings.web_enabled ? " checked" : ""} />
        Let Cordial use the web (search, fetch, browser)
      </label>
    </div>

    <h3 class="section-title">Model</h3>
    <div class="field">
      <div class="field__hint" style="font-size:12px">
        ${escapeHtml(model.description || "LFM3-VL-2B Q4_K_M")} · ${escapeHtml(model.backend.toUpperCase())} · llama.cpp ${escapeHtml(
          state.app.llama_tag || "—",
        )}
        <br />
        <span class="mono" style="font-size:10px">${escapeHtml(model.weights_path)}</span>
        <br />
        <span class="mono" style="font-size:10px">${escapeHtml(model.projector_path || "vision projector not downloaded")}</span>
      </div>
      <div class="download__actions">
        <button class="outline-button" data-action="open-workspace">Open workspace</button>
        <button class="outline-button" data-action="delete-model">Delete model files</button>
      </div>
    </div>

    <div class="settings__actions">
      <button class="ghost-button" data-action="close-settings">Cancel</button>
      <button class="primary-button" data-action="save-settings">Save changes</button>
    </div>

    <div class="about">
      Cordial ${escapeHtml(state.app.version)} · llama.cpp ${escapeHtml(state.app.llama_version || state.app.llama_tag)} · built with Rust and Tauri.
      <br />
      Weights: LFM3-VL-2B by Liquid AI, LFM Open License v1.0. Everything runs on this device.
    </div>
  </dialog>`;
}

/** Reads the dialog's fields back into a settings object. */
export function readSettingsForm(root: ParentNode, current: Settings): Settings {
  const value = (id: string): string =>
    (root.querySelector<HTMLInputElement>(`#${id}`)?.value ?? "").trim();

  const effort = value("setting-effort") as Effort;
  const permission = value("setting-permission") as PermissionMode;
  const context = Number.parseInt(value("setting-context"), 10);
  const threads = Number.parseInt(value("setting-threads"), 10);
  const web = root.querySelector<HTMLInputElement>("#setting-web")?.checked ?? true;

  return {
    ...current,
    effort: ["low", "medium", "high"].includes(effort) ? effort : current.effort,
    permission_mode: ["auto", "manual"].includes(permission) ? permission : current.permission_mode,
    context_tokens: Number.isFinite(context) ? Math.min(131072, Math.max(2048, context)) : current.context_tokens,
    threads: Number.isFinite(threads) ? Math.max(0, Math.min(64, threads)) : current.threads,
    workspace_dir: value("setting-workspace"),
    web_enabled: web,
  };
}
