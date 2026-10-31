import { escapeHtml, humanBytes } from "../format";
import { icons, logoMark } from "../icons";
import type { AppState } from "../store";

const WEIGHTS = "LFM3-VL-2B-Q4_K_M.gguf";
const PROJECTOR = "mmproj-LFM3-VL-2B-Q8_0.gguf";

export function downloadHtml(state: AppState): string {
  const progress = state.download;
  const total = progress?.total && progress.total > 0 ? progress.total : null;
  const share = progress && total ? Math.min(1, progress.received / total) : 0;
  const segments = 24;
  const filled = Math.round(share * segments);

  const bars = Array.from({ length: segments }, (_, index) =>
    `<span class="progress__bar"><span class="progress__fill" style="--fill:${index < filled ? 1 : 0}"></span></span>`,
  ).join("");

  const current = progress
    ? `${escapeHtml(progress.file)} · ${humanBytes(progress.received)}${total ? ` of ${humanBytes(total)}` : ""}`
    : state.downloadError
      ? `Download failed: ${escapeHtml(state.downloadError)}`
      : "Waiting to start";

  const downloading = Boolean(progress);
  const action = downloading
    ? `<button class="outline-button" data-action="close-window-hint" disabled>Downloading…</button>`
    : `<button class="primary-button" data-action="download">${icons.download(16)} ${
        state.downloadError ? "Try again" : "Download model"
      }</button>`;

  return `<div class="download">
    <div class="download__card">
      <span class="download__logo">${logoMark(40)}</span>
      <h1>Cordial</h1>
      <p>A private agent that runs entirely on this machine. The first run downloads LFM3-VL-2B at Q4_K_M — about 1.3 GB — then never phones home again.</p>

      <div class="progress" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(
        share * 100,
      )}" aria-label="Model download progress">${bars}</div>
      <div style="display:flex;justify-content:space-between;gap:16px;align-items:baseline">
        <span class="mono" style="font-size:12px">${current}</span>
        <span class="mono" style="font-size:12px">${progress ? `${Math.round(share * 100)}%` : ""}</span>
      </div>

      <div style="margin-top:20px">
        <div class="download__row"><span class="download__name">${WEIGHTS}</span><span class="mono">${humanBytes(
          1_100_000_000,
        )}</span></div>
        <div class="download__row"><span class="download__name">${PROJECTOR}</span><span class="mono">${humanBytes(
          203_000_000,
        )}</span></div>
        <div class="download__row"><span>Saved to</span><span class="mono">${escapeHtml(
          state.model.weights_path || "app data folder",
        )}</span></div>
      </div>

      <div class="download__actions">
        ${action}
        <button class="ghost-button" data-action="open-settings">Settings</button>
      </div>
      <p style="margin-top:16px;font-size:11px">Weights: LFM3-VL-2B by Liquid AI, LFM Open License v1.0. Inference: llama.cpp (MIT).</p>
    </div>
  </div>`;
}
