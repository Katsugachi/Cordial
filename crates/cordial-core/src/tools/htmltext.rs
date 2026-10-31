//! A small, dependency-free HTML to text converter.
//!
//! Pages arrive with script tags, inline styles and entity soup. The model only
//! needs readable prose and links, so this module throws away the rest and keeps
//! the structure that matters: headings, paragraphs, list items and links.

/// Converts an HTML document to readable text.
pub fn html_to_text(html: &str) -> String {
    // `to_ascii_lowercase` only touches ASCII bytes, so every index in
    // `lowered` lines up with `html`.
    let lowered = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len() / 2);
    let mut title = String::new();
    let mut chars = html.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        if ch != '<' {
            out.push(ch);
            continue;
        }

        let rest = &html[index + 1..];
        let raw_name = tag_name(rest);
        let name = raw_name.trim_start_matches('/');
        let closing = raw_name.starts_with('/');

        if rest.starts_with("!--") {
            if let Some(offset) = html[index..].find("-->") {
                skip_to(&mut chars, index + offset + 3);
            }
            continue;
        }

        match name {
            "script" | "style" | "noscript" | "svg" | "canvas" | "template" | "iframe" => {
                if !closing {
                    skip_element(&mut chars, html, &lowered, index, name);
                }
                continue;
            }
            "title" => {
                if !closing {
                    let text = capture_text(&mut chars, html, &lowered, index, "title");
                    if !text.trim().is_empty() {
                        title = text.trim().to_string();
                    }
                }
                continue;
            }
            "a" => {
                if !closing {
                    if let Some(href) = attribute(rest, "href") {
                        let text = capture_text(&mut chars, html, &lowered, index, "a");
                        let label = text.trim();
                        if !label.is_empty() {
                            out.push_str(label);
                            if !is_internal(&href) && !label.starts_with("http") {
                                out.push_str(" (");
                                out.push_str(&href);
                                out.push(')');
                            }
                        }
                        continue;
                    }
                }
            }
            _ => {}
        }

        match name {
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "tr"
            | "blockquote" | "figure" | "figcaption" | "dl" | "dt" | "dd" | "table" => {
                push_break(&mut out)
            }
            "br" => out.push('\n'),
            "hr" => out.push_str("\n---\n"),
            "li" => {
                push_break(&mut out);
                out.push_str("• ");
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                push_break(&mut out);
                out.push('\n');
            }
            "td" | "th" => out.push_str(" | "),
            _ => {}
        }
    }

    let mut text = tidy(&decode_entities(&out));
    if !title.is_empty() && !text.starts_with(&title) {
        text = if text.is_empty() {
            title
        } else {
            format!("{title}\n\n{text}")
        };
    }
    text
}

fn tag_name(rest: &str) -> String {
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        if c.is_whitespace() || c == '>' || c == '/' {
            end = i;
            break;
        }
        if i > 24 {
            end = i;
            break;
        }
    }
    rest[..end].to_ascii_lowercase()
}

fn push_break(out: &mut String) {
    if !out.ends_with('\n') {
        out.push('\n');
    }
}

fn skip_to(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>, target: usize) {
    while let Some((i, _)) = chars.peek().copied() {
        if i >= target {
            break;
        }
        chars.next();
    }
}

/// Skips everything up to and including `</name>`.
fn skip_element(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    html: &str,
    lowered: &str,
    start: usize,
    name: &str,
) {
    let closing = format!("</{name}");
    let end = match lowered[start..].find(&closing) {
        Some(offset) => {
            let absolute = start + offset;
            match html[absolute..].find('>') {
                Some(gt) => absolute + gt + 1,
                None => html.len(),
            }
        }
        None => html.len(),
    };
    skip_to(chars, end);
}

/// Collects text between the current tag and its closing tag.
fn capture_text(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    html: &str,
    lowered: &str,
    start: usize,
    name: &str,
) -> String {
    let closing = format!("</{name}");
    let end = match lowered[start..].find(&closing) {
        Some(offset) => start + offset,
        None => html.len().min(start + 4_000),
    };
    let inner_start = match html[start..].find('>') {
        Some(gt) => start + gt + 1,
        None => end,
    };
    let text = strip_tags(html.get(inner_start..end).unwrap_or(""));
    // Skip past the closing tag's '>'.
    let after = end.saturating_add(closing.len());
    let absolute_end = html[after.min(html.len())..]
        .find('>')
        .map(|gt| after + gt + 1)
        .unwrap_or(after);
    skip_to(chars, absolute_end.min(html.len()));
    text
}

fn strip_tags(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut in_tag = false;
    for ch in fragment.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lowered = tag.to_ascii_lowercase();
    let needle = format!("{name}=");
    let position = lowered.find(&needle)?;
    let after = tag[position + needle.len()..].trim_start();
    if let Some(rest) = after.strip_prefix('"') {
        return Some(rest[..rest.find('"')?].to_string());
    }
    if let Some(rest) = after.strip_prefix('\'') {
        return Some(rest[..rest.find('\'')?].to_string());
    }
    let end = after
        .find(|c: char| c.is_whitespace() || c == '>')
        .unwrap_or(after.len());
    Some(after[..end].to_string())
}

fn is_internal(href: &str) -> bool {
    href.starts_with('#') || href.starts_with("javascript:") || href.starts_with("mailto:")
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(position) = rest.find('&') {
        out.push_str(&rest[..position]);
        let tail = &rest[position..];
        let window = &tail[..tail.len().min(12)];
        let Some(semicolon) = window.find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semicolon];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "rsquo" => Some('\u{2019}'),
            "lsquo" => Some('\u{2018}'),
            "ldquo" => Some('\u{201c}'),
            "rdquo" => Some('\u{201d}'),
            _ => entity.strip_prefix('#').and_then(|number| {
                let code = if let Some(hex) = number.strip_prefix(['x', 'X']) {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    number.parse::<u32>().ok()
                };
                code.and_then(char::from_u32)
            }),
        };
        match decoded {
            Some(ch) => out.push(ch),
            None => {
                out.push('&');
                out.push_str(entity);
                out.push(';');
            }
        }
        rest = &tail[semicolon + 1..];
    }
    out.push_str(rest);
    out
}

fn tidy(input: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in input.lines() {
        let trimmed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            if matches!(lines.last(), Some(last) if last.is_empty()) {
                continue;
            }
            lines.push(String::new());
        } else {
            lines.push(trimmed);
        }
    }
    while matches!(lines.last(), Some(last) if last.is_empty()) {
        lines.pop();
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts_and_styles() {
        let html = "<html><head><title>Doc</title><style>body{color:red}</style></head>\
                    <body><script>alert('x')</script><p>Hello <b>world</b></p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Hello world"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("color:red"));
        assert!(text.starts_with("Doc"));
    }

    #[test]
    fn keeps_paragraph_breaks() {
        let text = html_to_text("<p>One</p><p>Two</p><p>Three</p>");
        assert_eq!(text, "One\nTwo\nThree");
    }

    #[test]
    fn renders_links_with_targets() {
        let text = html_to_text(r#"<p>Read <a href="https://example.com/x">the docs</a>.</p>"#);
        assert!(text.contains("the docs (https://example.com/x)"));
    }

    #[test]
    fn decodes_entities() {
        let text = html_to_text("<p>Tom &amp; Jerry &lt;3 &#8212; caf&#233;</p>");
        assert_eq!(text, "Tom & Jerry <3 — café");
    }

    #[test]
    fn lists_become_bullets() {
        let text = html_to_text("<ul><li>Alpha</li><li>Beta</li></ul>");
        assert!(text.contains("• Alpha"));
        assert!(text.contains("• Beta"));
    }

    #[test]
    fn nested_markup_is_flattened() {
        let html =
            "<div><section><h1>Title</h1><p>Body <span>text</span> here.</p></section></div>";
        let text = html_to_text(html);
        assert!(text.contains("Title"));
        assert!(text.contains("Body text here."));
    }

    #[test]
    fn unclosed_script_does_not_eat_everything_after_it() {
        let text = html_to_text("<script>var x = 1; <p>real</p>");
        assert!(!text.contains("var x"));
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(html_to_text(""), "");
    }
}
