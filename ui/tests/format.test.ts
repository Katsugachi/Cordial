import { describe, expect, it } from "vitest";

import {
  contextLabel,
  escapeHtml,
  excerpt,
  humanBytes,
  relativeDate,
  toolFamily,
  toolLabel,
} from "../src/format";

describe("humanBytes", () => {
  it("uses the right unit", () => {
    expect(humanBytes(999)).toBe("999 B");
    expect(humanBytes(1_100_000_000)).toBe("1.0 GB");
    expect(humanBytes(203_000_000)).toBe("193.6 MB");
  });
});

describe("contextLabel", () => {
  it("formats token counts", () => {
    expect(contextLabel(131072)).toBe("128k");
    expect(contextLabel(40960)).toBe("40k");
    expect(contextLabel(2048)).toBe("2k");
    expect(contextLabel(512)).toBe("512");
  });
});

describe("relativeDate", () => {
  const now = new Date("2026-10-31T12:00:00").getTime();

  it("labels today and yesterday", () => {
    expect(relativeDate(new Date("2026-10-31T09:12:00").getTime(), now)).toBe("Today 09:12");
    expect(relativeDate(new Date("2026-10-30T15:04:00").getTime(), now)).toBe("Yesterday 15:04");
  });

  it("falls back to a date", () => {
    expect(relativeDate(new Date("2026-03-12T08:00:00").getTime(), now)).toContain("12");
  });
});

describe("tool helpers", () => {
  it("maps tools to families", () => {
    expect(toolFamily("web_search")).toBe("search");
    expect(toolFamily("browser_click")).toBe("browser");
    expect(toolFamily("write_file")).toBe("write");
    expect(toolFamily("read_file")).toBe("files");
  });

  it("labels tools in plain words", () => {
    expect(toolLabel("web_fetch")).toBe("Read page");
    expect(toolLabel("unknown_tool")).toBe("unknown_tool");
  });
});

describe("text helpers", () => {
  it("escapes html", () => {
    expect(escapeHtml('<script>alert("x")</script>')).toBe(
      "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;",
    );
  });

  it("flattens excerpts", () => {
    expect(excerpt("a\n\nb   c", 40)).toBe("a b c");
    expect(excerpt("x".repeat(30), 10)).toBe("xxxxxxxxx…");
  });
});
