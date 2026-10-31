//! A small, dependency-free HTML to text converter.
//!
//! Pages arrive with script tags, inline styles and entity soup. The model only
//! needs readable prose and links, so this module throws away the rest and keeps
//! the structure that matters: headings, paragraphs, list items and links.
//!
//! The parser is deliberately simple and single-pass: find `<`, read the tag
//! name, decide whether the tag's inner text matters, and skip to the end of the
//! tag. Anything it does not understand is dropped rather than shown to the
//! model as markup.

/// Elements whose contents are never shown.
const DROP: [&str; 7] = ["script", "style", "noscript", "svg", "canvas", "template", "iframe"];

/// Elements that end a line of text.
const BLOCK: [&str; 17] = [
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "tr",
    "blockquote",
    "figure",
    "figcaption",
    "dl",
    "dt",
    "dd",
    "table",
    "ul",
    "ol",
];

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

        // Comments and doctypes.
        if rest.starts_with("!--") || rest.starts_with('!') {
            if let Some(offset) = html[index..].find('>') {
                skip_to(&mut chars, index + offset + 1);
            }
            continue;
        }

        let closing = rest.starts_with('/');
        let body = rest.trim_start_matches('/');
        let name = tag_name(body);

        // End of the tag itself, so its characters never reach the output.
        let tag_end = html[index..]
            .find('>')
            .map(|offset| index + offset + 1)
            .unwrap_or(html.len());

        if name.is_empty() {
            skip_to(&mut chars, tag_end);
            continue;
        }

        if DROP.contains(&name.as_str()) {
            if closing {
                skip_to(&mut chars, tag_end);
            } else {
                skip_element(&mut chars, html, &lowered, index, &name);
            }
            continue;
        }

        if closing {
            // A closing tag only ends the line.
            if BLOCK.contains(&name.as_str())
                || matches!(name.as_str(), "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
            {
                push_break(&mut out);
            }
            skip_to(&mut chars, tag_end);
            continue;
        }

        match name.as_str() {
            "title" => {
                let text = capture_text(&mut chars, html, &lowered, index, "title");
                if !text.trim().is_empty() {
                    title = text.trim().to_string();
                }
                continue;
            }
            "a" => {
                if let Some(href) = attribute(body, "href") {
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
            other if BLOCK.contains(&other) => push_break(&mut out),
            _ => {}
        }

        skip_to(&mut chars, tag_end);
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

/// Reads the element name at the start of `rest`.
fn tag_name(rest: &str) -> String {
    let mut end = rest.len();
    for (index, ch) in rest.char_indices() {
        if ch.is_whitespace() || ch == '>' || ch == '/' || index > 24 {
            end = index;
            break;
        }
    }
    rest[..end].to_ascii_lowercase()
}

fn push_break(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn skip_to(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>, target: usize) {
    while let Some((index, _)) = chars.peek().copied() {
        if index >= target {
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

/// Collects the text between an opening tag and its closing tag.
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
    let after = end.saturating_add(closing.len()).min(html.len());
    let absolute_end = html[after..]
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
        assert!(text.contains("Hello world"), "got: {text}");
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
        assert!(text.contains("the docs (https://example.com/x)"), "got: {text}");
    }

    #[test]
    fn decodes_entities() {
        let text = html_to_text("<p>Tom &amp; Jerry &lt;3 &#8212; caf&#233;</p>");
        assert_eq!(text, "Tom & Jerry <3 — café");
    }

    #[test]
    fn lists_become_bullets_without_duplicates() {
        let text = html_to_text("<ul><li>Alpha</li><li>Beta</li></ul>");
        assert_eq!(text, "• Alpha\n• Beta", "got: {text}");
    }

    #[test]
    fn nested_markup_is_flattened() {
        let html = "<div><section><h1>Title</h1><p>Body <span>text</span> here.</p></section></div>";
        let text = html_to_text(html);
        assert!(text.contains("Title"));
        assert!(text.contains("Body text here."));
    }

    #[test]
    fn tags_with_attributes_are_consumed() {
        let text =
            html_to_text(r#"<p class="lead" data-x="1">Read this</p><div id="x">then this</div>"#);
        assert_eq!(text, "Read this\nthen this");
    }

    #[test]
    fn closing_tags_leave_no_debris() {
        let text = html_to_text("<div><p>One</p></div><ul><li>Two</li></ul>");
        assert!(!text.contains('>'), "got: {text}");
        assert!(!text.contains('<'), "got: {text}");
        assert!(text.contains("One"));
        assert!(text.contains("• Two"));
    }

    #[test]
    fn unclosed_script_does_not_eat_everything_after_it() {
        let text = html_to_text("<script>var x = 1; <p>real</p>");
        assert!(!text.contains("var x"));
    }

    #[test]
    fn entities_survive_in_links() {
        let text = html_to_text(r#"<a href="https://example.com/?a=1&amp;b=2">search &amp; more</a>"#);
        assert!(text.contains("search & more"));
        assert!(text.contains("https://example.com/?a=1&amp;b=2"));
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(html_to_text(""), "");
    }
}
