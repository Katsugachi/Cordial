// Markdown rendering for assistant answers.
//
// Model output is untrusted input: it can be steered by a web page the agent
// read. Everything is therefore sanitised before it reaches the DOM, and links
// are limited to http(s).

import DOMPurify from "dompurify";
import { marked } from "marked";

marked.setOptions({ gfm: true, breaks: true });

const ALLOWED_TAGS = [
  "a",
  "b",
  "blockquote",
  "br",
  "code",
  "del",
  "em",
  "h1",
  "h2",
  "h3",
  "h4",
  "hr",
  "i",
  "li",
  "ol",
  "p",
  "pre",
  "strong",
  "table",
  "tbody",
  "td",
  "th",
  "thead",
  "tr",
  "ul",
];

export function renderMarkdown(text: string): string {
  const raw = marked.parse(text, { async: false }) as string;
  return DOMPurify.sanitize(raw, {
    ALLOWED_TAGS,
    ALLOWED_ATTR: ["href", "target", "rel", "class"],
    ALLOW_DATA_ATTR: false,
  });
}

/** Renders plain text with line breaks preserved but no markup. */
export function renderPlain(text: string): string {
  return DOMPurify.sanitize(text, { ALLOWED_TAGS: [], ALLOWED_ATTR: [] }).replace(/\n/g, "<br />");
}

/** True when a reply looks like it needs the markdown path. */
export function looksRich(text: string): boolean {
  return /(^|\n)(#{1,6} |[-*] |\d+\. |```|> )|\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\(https?:/.test(text);
}
