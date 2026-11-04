import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { mount } from "../src/app";
import { createBridge } from "../src/bridge";

vi.mock("../src/pdf", () => ({
  MAX_PDF_BYTES: 10_000_000,
  preparePdf: vi.fn(async () => [
    { text: "Question 1", image: "data:image/png;base64,AAAA" },
  ]),
}));

beforeEach(() => vi.useFakeTimers());
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); document.body.innerHTML = ""; });

it("keeps both image and text for a PDF page and sends them together", async () => {
  document.body.innerHTML = '<div id="app"></div>';
  const root = document.getElementById("app")!;
  const bridge = createBridge();
  const sent = vi.spyOn(bridge, "sendMessage");
  mount(root, bridge);
  await vi.advanceTimersByTimeAsync(20);
  let picker: HTMLInputElement | undefined;
  const original = document.createElement.bind(document);
  vi.spyOn(document, "createElement").mockImplementation(((tag: string) => {
    const element = original(tag);
    if (tag === "input") {
      picker = element as HTMLInputElement;
      vi.spyOn(picker, "click").mockImplementation(() => {});
    }
    return element;
  }) as typeof document.createElement);
  root.querySelector<HTMLElement>('[data-action="attach-pdf"]')!.click();
  const file = new File(["%PDF-mock"], "worksheet.pdf", { type: "application/pdf" });
  Object.defineProperty(file, "arrayBuffer", { value: async () => new Uint8Array([37, 80, 68, 70, 45]).buffer });
  Object.defineProperty(picker!, "files", { value: [file] });
  picker!.dispatchEvent(new Event("change"));
  await vi.advanceTimersByTimeAsync(30);
  expect(root.textContent).toContain("worksheet.pdf, page 1 (image)");
  expect(root.textContent).toContain("worksheet.pdf, page 1 (text)");
  root.querySelector<HTMLTextAreaElement>("#input")!.value = "Complete this worksheet";
  root.querySelector<HTMLElement>('[data-action="send"]')!.click();
  await vi.advanceTimersByTimeAsync(50);
  expect(sent).toHaveBeenCalledTimes(1);
  const attachments = sent.mock.calls[0][2];
  expect(attachments).toHaveLength(2);
  expect(attachments[0].inline_data).toBe("data:image/png;base64,AAAA");
  expect(attachments[1].inline_data).toBe("Question 1");
});

it("does not attach a PDF to another conversation if the user switches during preparation", async () => {
  document.body.innerHTML = '<div id="app"></div>';
  const root = document.getElementById("app")!;
  mount(root, createBridge());
  await vi.advanceTimersByTimeAsync(20);
  let picker: HTMLInputElement | undefined;
  const original = document.createElement.bind(document);
  vi.spyOn(document, "createElement").mockImplementation(((tag: string) => {
    const element = original(tag);
    if (tag === "input") {
      picker = element as HTMLInputElement;
      vi.spyOn(picker, "click").mockImplementation(() => {});
    }
    return element;
  }) as typeof document.createElement);
  root.querySelector<HTMLElement>('[data-action="attach-pdf"]')!.click();
  let release!: (data: ArrayBuffer) => void;
  const file = new File(["%PDF-mock"], "delayed.pdf", { type: "application/pdf" });
  Object.defineProperty(file, "arrayBuffer", { value: () => new Promise<ArrayBuffer>((resolve) => { release = resolve; }) });
  Object.defineProperty(picker!, "files", { value: [file] });
  picker!.dispatchEvent(new Event("change"));
  root.querySelector<HTMLElement>('[data-action="new-session"]')!.click();
  await vi.advanceTimersByTimeAsync(20);
  release(new Uint8Array([37, 80, 68, 70, 45]).buffer);
  await vi.advanceTimersByTimeAsync(30);
  expect(root.textContent).toContain("The conversation changed while the PDF was being prepared");
  expect(root.querySelectorAll(".chip__name")).toHaveLength(0);
});

it("clears prepared attachments when opening another conversation", async () => {
  document.body.innerHTML = '<div id="app"></div>';
  const root = document.getElementById("app")!;
  mount(root, createBridge());
  await vi.advanceTimersByTimeAsync(20);
  root.querySelector<HTMLElement>('[data-action="new-session"]')!.click();
  await vi.advanceTimersByTimeAsync(20);
  let picker: HTMLInputElement | undefined;
  const original = document.createElement.bind(document);
  vi.spyOn(document, "createElement").mockImplementation(((tag: string) => {
    const element = original(tag);
    if (tag === "input") {
      picker = element as HTMLInputElement;
      vi.spyOn(picker, "click").mockImplementation(() => {});
    }
    return element;
  }) as typeof document.createElement);
  root.querySelector<HTMLElement>('[data-action="attach-pdf"]')!.click();
  const file = new File(["%PDF-mock"], "private.pdf", { type: "application/pdf" });
  Object.defineProperty(file, "arrayBuffer", { value: async () => new Uint8Array([37, 80, 68, 70, 45]).buffer });
  Object.defineProperty(picker!, "files", { value: [file] });
  picker!.dispatchEvent(new Event("change"));
  await vi.advanceTimersByTimeAsync(30);
  expect(root.textContent).toContain("private.pdf, page 1 (image)");
  root.querySelector<HTMLElement>('[data-action="open-session"][data-id="demo-session"]')!.click();
  await vi.advanceTimersByTimeAsync(20);
  expect(root.querySelectorAll(".chip__name")).toHaveLength(0);
});
