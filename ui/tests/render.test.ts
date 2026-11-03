import { describe, expect, it } from "vitest";

import { renderMarkdown } from "../src/markdown";
import { initialState } from "../src/store";
import type { Message, Session } from "../src/types";
import { chatHtml, visibleStreamText } from "../src/views/chat";
import { downloadHtml } from "../src/views/download";
import { composerHtml } from "../src/views/composer";

function sessionWith(messages: Message[]): Session {
  return { id: "s1", title: "Test", created_at: 0, updated_at: 0, messages };
}

const userMessage: Message = {
  id: "u1",
  role: "user",
  content: "Is it going to rain?",
  tool_calls: [],
  tool_call_id: null,
  attachments: [],
  created_at: Date.now(),
};

describe("markdown", () => {
  it("renders headings, lists and code", () => {
    const html = renderMarkdown("# Title\n\n- one\n- two\n\n`code`");
    expect(html).toContain("<h1>Title</h1>");
    expect(html).toContain("<li>one</li>");
    expect(html).toContain("<code>code</code>");
  });

  it("removes scripts and event handlers", () => {
    const html = renderMarkdown('<script>alert(1)</script><img src=x onerror="alert(1)">');
    expect(html).not.toContain("<script");
    expect(html).not.toContain("onerror");
  });

  it("keeps http links", () => {
    const html = renderMarkdown("[docs](https://example.com/page)");
    expect(html).toContain('href="https://example.com/page"');
  });
});

describe("chat view", () => {
  it("does not stream raw tool arguments as an answer", () => {
    expect(visibleStreamText('{"query":"llama.cpp"}')).toBe("");
    expect(visibleStreamText('<tool_call>secret')).toBe("");
    expect(visibleStreamText('Looking up<tool_call>secret')).toBe('Looking up');
    expect(visibleStreamText('A normal answer')).toBe('A normal answer');
  });

  it("shows the empty state for a fresh session", () => {
    const state = { ...initialState(), session: sessionWith([]) };
    const html = chatHtml(state);
    expect(html).toContain("What should we look into?");
    expect(html).toContain('data-action="suggestion"');
  });

  it("renders user text escaped and assistant markdown", () => {
    const assistant: Message = {
      id: "a1",
      role: "assistant",
      content: "**Yes** — bring an umbrella.",
      thinking: "Checking the radar.",
      tool_calls: [],
      tool_call_id: null,
      attachments: [],
      created_at: Date.now(),
    };
    const state = {
      ...initialState(),
      session: sessionWith([{ ...userMessage, content: "<b>rain?</b>" }, assistant]),
    };
    const html = chatHtml(state);
    expect(html).toContain("&lt;b&gt;rain?&lt;/b&gt;");
    expect(html).toContain("<strong>Yes</strong>");
    expect(html).toContain("Reasoning");
  });

  it("renders tool cards for a tool call with its result", () => {
    const assistant: Message = {
      id: "a2",
      role: "assistant",
      content: "",
      thinking: null,
      tool_calls: [
        { id: "c1", name: "web_search", arguments: [{ name: "query", value: "rain sydney" }] },
      ],
      tool_call_id: null,
      attachments: [],
      created_at: Date.now(),
    };
    const tool: Message = {
      id: "t1",
      role: "tool",
      content: "1. BOM — https://bom.gov.au",
      tool_calls: [],
      tool_call_id: "c1",
      attachments: [],
      created_at: Date.now(),
    };
    const state = { ...initialState(), session: sessionWith([userMessage, assistant, tool]) };
    const html = chatHtml(state);
    expect(html).toContain('data-family="search"');
    expect(html).toContain("Search");
    expect(html).toContain("Done");
    expect(html).toContain("bom.gov.au");
  });

  it("does not label an unfinished tool as Running after the turn ends", () => {
    const assistant: Message = {
      id: "a1", role: "assistant", content: "", thinking: null, attachments: [], created_at: 1,
      tool_calls: [{ id: "c1", name: "read_page", arguments: [{ name: "url", value: "https://github.com/Katsugachi" }] }],
    };
    const state = { ...initialState(), session: sessionWith([userMessage, assistant]), busy: false };
    const html = chatHtml(state);
    expect(html).toContain('data-status="interrupted"');
    expect(html).toContain("Interrupted");
    expect(html).not.toContain('data-status="running"');
  });

  it("shows the approval banner with both choices", () => {
    const state = {
      ...initialState(),
      session: sessionWith([userMessage]),
      approval: {
        id: "c9",
        name: "write_file",
        preview: "write_file(path=plan.md)",
        risk: "writes local files",
        status: "awaiting" as const,
        summary: "",
        detail: "",
        thumbnail: null,
      },
    };
    const html = chatHtml(state);
    expect(html).toContain('data-action="approve"');
    expect(html).toContain('data-allow="false"');
    expect(html).toContain("writes local files");
  });
});

describe("composer", () => {
  it("disables sending through the stop button while busy", () => {
    const idle = composerHtml({ ...initialState(), phase: "ready" }, []);
    expect(idle).toContain('data-action="send"');
    const busy = composerHtml({ ...initialState(), phase: "ready", busy: true }, []);
    expect(busy).toContain('data-action="stop"');
  });

  it("lists attachments with remove controls", () => {
    const html = composerHtml({ ...initialState(), phase: "ready" }, [
      { kind: "image", name: "chart.png", path: "/tmp/chart.png" },
    ]);
    expect(html).toContain("chart.png");
    expect(html).toContain('data-action="remove-attachment"');
  });
});

describe("interface copy", () => {
  it("keeps interface labels free of em dashes", () => {
    const state = initialState();
    expect(downloadHtml(state)).not.toContain("—");
    expect(composerHtml(state, [])).not.toContain("—");
    expect(chatHtml(state)).not.toContain("—");
  });
});

describe("download screen", () => {
  it("explains the first run and offers the download", () => {
    const html = downloadHtml(initialState());
    expect(html).toContain("Download model");
    expect(html).toContain("LFM3-VL-2B-Q4_K_M.gguf");
    expect(html).toContain("LFM Open License");
  });

  it("shows progress while downloading", () => {
    const state = {
      ...initialState(),
      download: { file: "LFM3-VL-2B-Q4_K_M.gguf", received: 550_000_000, total: 1_100_000_000 },
    };
    const html = downloadHtml(state);
    expect(html).toContain('role="progressbar"');
    expect(html).toContain("50%");
  });
});

describe("task checklist", () => {
  it("renders progress and escapes untrusted step text", () => {
    const session = { ...sessionWith([userMessage]), plan: [
      { id: 0, title: "<script>bad</script>", status: "done" as const, note: "checked" },
      { id: 1, title: "Next", status: "running" as const, note: "" },
    ] };
    const html = chatHtml({ ...initialState(), session });
    expect(html).toContain("1/2");
    expect(html).toContain("&lt;script&gt;bad&lt;/script&gt;");
    expect(html).not.toContain("<script>");
  });
});
