// Small display helpers. Pure functions so they can be unit tested.

/** 1_100_000_000 -> "1.0 GB" */
export function humanBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${Math.round(value)} ${units[0]}` : `${value.toFixed(1)} ${units[unit]}`;
}

/** 131072 -> "128k" */
export function contextLabel(tokens: number): string {
  if (tokens >= 1024) {
    const k = tokens / 1024;
    return `${Number.isInteger(k) ? k : k.toFixed(1)}k`;
  }
  return String(tokens);
}

/** Transfers per second -> "12.4 MB/s" */
export function speedLabel(bytesPerSecond: number): string {
  if (bytesPerSecond <= 0) return "";
  return `${humanBytes(bytesPerSecond)}/s`;
}

/** "2026-10-31T09:12:00Z" -> "Today 09:12", "Yesterday 15:04", "12 Mar" */
export function relativeDate(timestampMs: number, now = Date.now()): string {
  const date = new Date(timestampMs);
  const startOfToday = new Date(now);
  startOfToday.setHours(0, 0, 0, 0);
  const startOfYesterday = new Date(startOfToday.getTime() - 86_400_000);
  const time = `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
  if (date >= startOfToday) return `Today ${time}`;
  if (date >= startOfYesterday) return `Yesterday ${time}`;
  const month = date.toLocaleString(undefined, { month: "short" });
  return `${date.getDate()} ${month}`;
}

/** Escapes text for safe interpolation into HTML. */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/** One-line excerpt used in tool cards. */
export function excerpt(text: string, limit = 160): string {
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= limit) return flat;
  return `${flat.slice(0, limit - 1)}…`;
}

/** Tool family drives the card colour. */
export function toolFamily(name: string): "search" | "browser" | "files" | "write" {
  if (name === "web_search") return "search";
  if (name.startsWith("browser_")) return "browser";
  if (name === "write_file" || name === "docx_edit" || name === "docx_create") return "write";
  return "files";
}

/** Human label for a tool name. */
export function toolLabel(name: string): string {
  switch (name) {
    case "web_search":
      return "Search";
    case "web_fetch":
      return "Read page";
    case "browser_open":
      return "Open page";
    case "browser_click":
      return "Click";
    case "browser_type":
      return "Type";
    case "browser_view":
      return "Look";
    case "read_file":
      return "Read file";
    case "list_files":
      return "List files";
    case "write_file":
      return "Write file";
    case "run_command":
      return "Run command";
    case "docx_read":
      return "Read Word file";
    case "docx_edit":
      return "Edit Word file";
    case "docx_create":
      return "Create Word file";
    case "update_plan":
      return "Update checklist";
    default:
      return name;
  }
}
