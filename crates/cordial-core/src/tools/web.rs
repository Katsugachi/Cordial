//! Web search and page fetching.
//!
//! Search uses DuckDuckGo's HTML endpoints, which need no API key and therefore
//! keep Cordial fully local and free to run. The parser is defensive: search
//! engines change markup and rate-limit aggressively, so every failure turns into
//! a sentence the model can act on rather than an exception.

use std::collections::HashSet;

use scraper::{Html, Selector};
use serde::Deserialize;

use crate::error::CoreError;
use crate::session::ToolCall;
use crate::tools::htmltext::html_to_text;
use crate::tools::{ToolOutcome, ToolRuntime};
use crate::util::truncate_chars;

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub async fn search(runtime: &ToolRuntime, call: &ToolCall) -> ToolOutcome {
    let Some(query) = call.arg("query").map(str::trim).filter(|q| !q.is_empty()) else {
        return ToolOutcome::fail("missing query", "Error: `query` is required.");
    };

    match run_search(runtime, query).await {
        Ok(hits) if hits.is_empty() => ToolOutcome::ok(
            format!("no results for “{query}”"),
            format!(
                "No results for “{query}”. Try different keywords, or use `web_fetch` with a \
                 direct link if you already know one."
            ),
        ),
        Ok(hits) => {
            let limit = runtime.effort().search_results().min(hits.len());
            let mut text = format!("Search results for “{query}”:\n");
            for (index, hit) in hits.iter().take(limit).enumerate() {
                text.push_str(&format!(
                    "\n{}. {}\n{}\n{}\n",
                    index + 1,
                    hit.title,
                    hit.url,
                    truncate_chars(&hit.snippet, 320)
                ));
            }
            text.push_str(
                "\nOpen a result with `web_fetch` or `browser_open` when you need the details.",
            );
            ToolOutcome::ok(format!("{} results for “{query}”", limit), text)
        }
        Err(err) => ToolOutcome::fail(
            "search unavailable",
            format!(
                "Error: the search service could not be reached ({err}).\
                 \nYou can still use `web_fetch` on a known URL, or answer from what you know \
                 and say that you could not check the web."
            ),
        ),
    }
}

async fn run_search(runtime: &ToolRuntime, query: &str) -> Result<Vec<SearchHit>, CoreError> {
    // Primary: the HTML endpoint. Fallback: the lite endpoint.
    let primary = runtime
        .http()
        .post("https://html.duckduckgo.com/html/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("referer", "https://duckduckgo.com/")
        .body(format!("q={}&kl=wt-wt", urlencode(query)))
        .send()
        .await;

    if let Ok(response) = primary {
        if response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            let hits = parse_html_results(&body);
            if !hits.is_empty() {
                return Ok(hits);
            }
        }
    }

    let response = runtime
        .http()
        .post("https://lite.duckduckgo.com/lite/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("referer", "https://lite.duckduckgo.com/")
        .body(format!("q={}", urlencode(query)))
        .send()
        .await
        .map_err(|e| CoreError::Network(e.to_string()))?;

    if !response.status().is_success() {
        return Err(CoreError::Network(format!(
            "search returned HTTP {}",
            response.status()
        )));
    }
    let body = response.text().await.unwrap_or_default();
    let hits = parse_lite_results(&body);
    if hits.is_empty() {
        return Err(CoreError::Network(
            "the search page contained no results (rate limited?)".to_string(),
        ));
    }
    Ok(hits)
}

fn parse_html_results(body: &str) -> Vec<SearchHit> {
    let document = Html::parse_document(body);
    let Ok(block) = Selector::parse("div.result") else {
        return Vec::new();
    };
    let Ok(link) = Selector::parse("a.result__a") else {
        return Vec::new();
    };
    let Ok(snippet) = Selector::parse(".result__snippet") else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for element in document.select(&block) {
        let Some(anchor) = element.select(&link).next() else {
            continue;
        };
        let title = clean(&anchor.text().collect::<Vec<_>>().join(" "));
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let url = unwrap_redirect(href);
        if title.is_empty() || url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        let snippet_text = element
            .select(&snippet)
            .next()
            .map(|s| clean(&s.text().collect::<Vec<_>>().join(" ")))
            .unwrap_or_default();
        hits.push(SearchHit {
            title,
            url,
            snippet: snippet_text,
        });
    }
    hits
}

fn parse_lite_results(body: &str) -> Vec<SearchHit> {
    let document = Html::parse_document(body);
    let Ok(link) = Selector::parse("a.result-link") else {
        return Vec::new();
    };
    let Ok(snippet) = Selector::parse("td.result-snippet") else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    let snippets: Vec<String> = document
        .select(&snippet)
        .map(|s| clean(&s.text().collect::<Vec<_>>().join(" ")))
        .collect();

    for (index, anchor) in document.select(&link).enumerate() {
        let title = clean(&anchor.text().collect::<Vec<_>>().join(" "));
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let url = unwrap_redirect(href);
        if title.is_empty() || url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        hits.push(SearchHit {
            title,
            url,
            snippet: snippets.get(index).cloned().unwrap_or_default(),
        });
    }
    hits
}

/// DuckDuckGo wraps outbound links as `/l/?uddg=<encoded>`.
fn unwrap_redirect(href: &str) -> String {
    let candidate = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_string()
    };
    if let Ok(parsed) = url::Url::parse(&candidate) {
        if let Some((_, value)) = parsed
            .query_pairs()
            .find(|(key, _)| key == "uddg" || key == "u")
        {
            let decoded = value.to_string();
            if decoded.starts_with("http") {
                return decoded;
            }
        }
    }
    candidate
}

fn clean(text: &str) -> String {
    crate::util::collapse_whitespace(&html_to_text_entities(text))
}

fn html_to_text_entities(text: &str) -> String {
    // Snippets arrive partially encoded; reuse the converter's entity pass by
    // wrapping the fragment in a paragraph.
    html_to_text(&format!("<p>{text}</p>"))
}

fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b' ' => {
                if *byte == b' ' {
                    out.push('+');
                } else {
                    out.push(*byte as char);
                }
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub async fn fetch(runtime: &ToolRuntime, call: &ToolCall) -> ToolOutcome {
    let Some(raw_url) = call.arg("url").map(str::trim).filter(|u| !u.is_empty()) else {
        return ToolOutcome::fail("missing url", "Error: `url` is required.");
    };
    let Some(url) = normalise_url(raw_url) else {
        return ToolOutcome::fail(
            "invalid url",
            format!("Error: `{raw_url}` is not a valid http or https link."),
        );
    };

    let response = match runtime.http().get(&url).send().await {
        Ok(response) => response,
        Err(err) => {
            return ToolOutcome::fail(
                "fetch failed",
                format!("Error: could not load {url}: {err}"),
            )
        }
    };

    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if !status.is_success() {
        return ToolOutcome::fail(
            format!("HTTP {}", status.as_u16()),
            format!(
                "Error: {url} returned HTTP {}. Try another link, or search again.",
                status.as_u16()
            ),
        );
    }
    if content_type.contains("pdf") {
        return ToolOutcome::fail(
            "pdf not supported",
            format!("Error: {url} is a PDF. Cordial cannot read PDFs; look for an HTML version."),
        );
    }
    if content_type.starts_with("image/") {
        return ToolOutcome::fail(
            "image not text",
            format!("Error: {url} is an image. Use `browser_open` if you need to look at a page."),
        );
    }

    let body = match response.text().await {
        Ok(body) => body,
        Err(err) => return ToolOutcome::fail("read failed", format!("Error: {err}")),
    };

    let text = if content_type.contains("html") || body.trim_start().starts_with('<') {
        html_to_text(&body)
    } else if content_type.contains("json") {
        match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(value) => serde_json::to_string_pretty(&value).unwrap_or(body.clone()),
            Err(_) => body.clone(),
        }
    } else {
        body.clone()
    };

    let limit = runtime.effort().fetch_chars();
    let clipped = truncate_chars(&text, limit);
    let summary = format!("{} chars from {}", text.chars().count(), short_host(&url));
    ToolOutcome::ok(summary, format!("Text of {url}:\n\n{clipped}"))
}

fn normalise_url(raw: &str) -> Option<String> {
    let candidate = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else if raw.contains('.') && !raw.contains(' ') {
        format!("https://{raw}")
    } else {
        return None;
    };
    let parsed = url::Url::parse(&candidate).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    Some(parsed.to_string())
}

fn short_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckduckgo_redirects_are_unwrapped() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&rut=abc";
        assert_eq!(unwrap_redirect(href), "https://example.com/page");
    }

    #[test]
    fn direct_links_pass_through() {
        assert_eq!(
            unwrap_redirect("https://example.org/a?b=c"),
            "https://example.org/a?b=c"
        );
    }

    #[test]
    fn html_results_are_parsed() {
        let body = r#"
        <html><body>
          <div class="result results_links">
            <h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa">Example A</a></h2>
            <a class="result__snippet">First snippet &amp; more</a>
          </div>
          <div class="result results_links">
            <h2><a class="result__a" href="https://example.org/b">Example B</a></h2>
            <a class="result__snippet">Second snippet</a>
          </div>
        </body></html>"#;
        let hits = parse_html_results(body);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.com/a");
        assert_eq!(hits[0].title, "Example A");
        assert!(hits[0].snippet.contains("First snippet"));
        assert_eq!(hits[1].url, "https://example.org/b");
    }

    #[test]
    fn lite_results_are_parsed() {
        let body = r#"
        <html><body><table>
          <tr><td><a class="result-link" href="https://lite.example/1">Lite one</a></td></tr>
          <tr><td class="result-snippet">Snippet one</td></tr>
          <tr><td><a class="result-link" href="https://lite.example/2">Lite two</a></td></tr>
          <tr><td class="result-snippet">Snippet two</td></tr>
        </table></body></html>"#;
        let hits = parse_lite_results(body);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].snippet, "Snippet two");
    }

    #[test]
    fn urls_are_normalised() {
        assert_eq!(
            normalise_url("example.com"),
            Some("https://example.com/".to_string())
        );
        assert_eq!(normalise_url("not a url"), None);
        assert_eq!(normalise_url("ftp://example.com"), None);
    }

    #[test]
    fn query_encoding_handles_spaces_and_symbols() {
        assert_eq!(urlencode("sydney weather"), "sydney+weather");
        assert_eq!(urlencode("a&b"), "a%26b");
    }
}
