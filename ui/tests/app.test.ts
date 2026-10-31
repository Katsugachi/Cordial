// Integration tests for the whole window: mount, boot, send, stream, approve.
//
// The demo bridge stands in for Tauri, so this exercises the same code paths the
// desktop app uses without needing a webview or a model.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { mount } from "../src/app";
import { createBridge } from "../src/bridge";

async function bootApp(): Promise<HTMLElement> {
  document.body.innerHTML = '<div id="app"></div>';
  const root = document.getElementById("app") as HTMLElement;
  mount(root, createBridge());
  // Let the bootstrap promise chain settle.
  await vi.advanceTimersByTimeAsync(20);
  return root;
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("window", () => {
  it("boots into the demo and shows the sidebar, demo label and transcript", async () => {
    const root = await bootApp();
    expect(root.querySelector(".sidebar")).not.toBeNull();
    expect(root.querySelector(".topbar__title")?.textContent).toBe("Tide times in Manly");
    expect(root.textContent).toContain("Demo preview");
    expect(root.textContent).toContain("high tide at Manly");
    expect(root.querySelector("#input")).not.toBeNull();
  });

  it("streams a tool call and an answer when you send a message", async () => {
    const root = await bootApp();
    const input = root.querySelector<HTMLTextAreaElement>("#input") as HTMLTextAreaElement;
    input.value = "What is the tide time today?";

    root.querySelector<HTMLElement>('[data-action="send"]')?.click();
    await vi.advanceTimersByTimeAsync(60);

    // The turn started and the search card appeared.
    expect(root.textContent).toContain("Search");

    await vi.advanceTimersByTimeAsync(4000);

    expect(root.textContent).toContain("09:42");
    expect(root.querySelector('[data-streaming="true"]')).toBeNull();
    expect(root.querySelectorAll(".msg").length).toBeGreaterThanOrEqual(4);
  });

  it("shows the stop control while answering and clears it afterwards", async () => {
    const root = await bootApp();
    root.querySelector<HTMLTextAreaElement>("#input")!.value = "Tell me something";
    root.querySelector<HTMLElement>('[data-action="send"]')?.click();
    await vi.advanceTimersByTimeAsync(60);

    expect(root.querySelector('[data-action="stop"]')).not.toBeNull();

    await vi.advanceTimersByTimeAsync(5000);
    expect(root.querySelector('[data-action="stop"]')).toBeNull();
    expect(root.querySelector('[data-action="send"]')).not.toBeNull();
  });

  it("switches effort from the header", async () => {
    const root = await bootApp();
    const high = root.querySelector<HTMLElement>('[data-action="set-effort"][data-value="high"]');
    high?.click();
    await vi.advanceTimersByTimeAsync(20);
    const pressed = root
      .querySelector<HTMLElement>('[data-action="set-effort"][data-value="high"]')
      ?.getAttribute("aria-pressed");
    expect(pressed).toBe("true");
  });

  it("opens and closes the settings dialog", async () => {
    const root = await bootApp();
    root.querySelector<HTMLElement>('[data-action="open-settings"]')?.click();
    await vi.advanceTimersByTimeAsync(10);

    const dialog = document.querySelector<HTMLDialogElement>("#settings-dialog");
    expect(dialog).not.toBeNull();
    expect(document.body.textContent).toContain("Save changes");

    dialog?.querySelector<HTMLElement>('[data-action="close-settings"]')?.click();
    expect(document.querySelector("#settings-dialog")).toBeNull();
  });

  it("creates a new chat from the sidebar", async () => {
    const root = await bootApp();
    root.querySelector<HTMLElement>('[data-action="new-session"]')?.click();
    await vi.advanceTimersByTimeAsync(20);

    expect(root.querySelector(".topbar__title")?.textContent).toBe("New chat");
    expect(root.textContent).toContain("What should we look into?");
  });

  it("sends a suggested prompt with one click", async () => {
    const root = await bootApp();
    root.querySelector<HTMLElement>('[data-action="new-session"]')?.click();
    await vi.advanceTimersByTimeAsync(20);

    root.querySelector<HTMLElement>('[data-action="suggestion"]')?.click();
    await vi.advanceTimersByTimeAsync(5000);

    expect(root.querySelectorAll(".msg").length).toBeGreaterThanOrEqual(2);
  });
});
