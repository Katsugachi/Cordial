//! Web search and page fetching.
//!
//! Search uses DuckDuckGo's HTML endpoints, which need no API key and therefore
//! keep Cordial fully local and free to run. The parser is defensive: search
//! engines change markup and rate-limit aggressively. If both DuckDuckGo
//! endpoints fail, a clearly labelled Wikipedia-only search is attempted.

use std::collections::HashSet;

use scraper::{Html, Selector};
use serde::Deserialize;

use crate::error::CoreError;
use crate::session::ToolCall;
use crate::tools::htmltext::html_to_text;
use crate::tools::{ToolOutcome, ToolRuntime};
use crate::util::truncate_chars;

// Bound decoded HTTP bodies before parsing, including responses with no or a
// misleading Content-Length. This is not a network isolation mechanism.
const MAX_WEB_BODY_BYTES: usize = 2_000_000;

fn append_bounded(buffer: &mut Vec<u8>, chunk: &[u8]) -> Result<(), CoreError> {
    if chunk.len() > MAX_WEB_BODY_BYTES.saturating_sub(buffer.len()) {
        return Err(CoreError::Network(
            "response exceeds the web body limit".to_string(),
        ));
    }
    buffer.extend_from_slice(chunk);
    Ok(())
}

async fn read_bounded(mut response: reqwest::Response) -> Result<String, CoreError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_WEB_BODY_BYTES as u64)
    {
        return Err(CoreError::Network(
            "response exceeds the web body limit".to_string(),
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| CoreError::Network(err.to_string()))?
    {
        append_bounded(&mut bytes, &chunk)?;
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

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
        Ok((hits, wikipedia_only)) if hits.is_empty() => ToolOutcome::ok(
            format!("no {}results for “{query}”", if wikipedia_only { "Wikipedia " } else { "" }),
            format!(
                "{}No results for “{query}”. Try different keywords, or use `web_fetch` with a direct link if you already know one.",
                if wikipedia_only { "DuckDuckGo was unavailable; Wikipedia-only fallback: " } else { "" }
            ),
        ),
        Ok((hits, wikipedia_only)) => {
            let limit = runtime.effort().search_results().min(hits.len());
            let mut text = if wikipedia_only {
                format!("Wikipedia-only fallback results for “{query}”; these are not general web search results:\n")
            } else {
                format!("Search results for “{query}”:\n")
            };
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
            ToolOutcome::ok(format!("{} {}results for “{query}”", limit, if wikipedia_only { "Wikipedia " } else { "" }), text)
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

async fn run_search(
    runtime: &ToolRuntime,
    query: &str,
) -> Result<(Vec<SearchHit>, bool), CoreError> {
    if let Ok(hits) = run_duckduckgo_search(runtime, query).await {
        return Ok((hits, false));
    }
    let response = runtime
        .http()
        .get("https://en.wikipedia.org/w/api.php")
        .query(&[
            ("action", "query"),
            ("list", "search"),
            ("format", "json"),
            ("utf8", "1"),
            ("srlimit", "10"),
            ("srsearch", query),
        ])
        .send()
        .await
        .map_err(|err| CoreError::Network(err.to_string()))?;
    if !response.status().is_success() {
        return Err(CoreError::Network(format!(
            "Wikipedia fallback returned HTTP {}",
            response.status()
        )));
    }
    let hits = parse_wikipedia_results(&read_bounded(response).await?)?;
    Ok((hits, true))
}

fn parse_wikipedia_results(body: &str) -> Result<Vec<SearchHit>, CoreError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|err| CoreError::Network(format!("invalid Wikipedia search response: {err}")))?;
    let entries = value
        .pointer("/query/search")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CoreError::Network("invalid Wikipedia search response".to_string()))?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let pageid = entry.get("pageid")?.as_u64()?;
            let title = truncate_chars(entry.get("title")?.as_str()?, 200);
            let snippet = clean(entry.get("snippet").and_then(|v| v.as_str()).unwrap_or(""));
            Some(SearchHit {
                title,
                url: format!("https://en.wikipedia.org/?curid={pageid}"),
                snippet,
            })
        })
        .collect())
}

async fn run_duckduckgo_search(
    runtime: &ToolRuntime,
    query: &str,
) -> Result<Vec<SearchHit>, CoreError> {
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
            let body = read_bounded(response).await.unwrap_or_default();
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
    let body = read_bounded(response).await?;
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

    let body = match read_bounded(response).await {
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
    let trimmed = raw.trim();
    let candidate = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else if trimmed.contains("://") {
        // Something like "ftp://host/page". Prefixing it with https would make
        // `ftp` the host and `//host/page` the path, which is not what the user
        // or the model meant.
        return None;
    } else if trimmed.contains('.') && !trimmed.chars().any(char::is_whitespace) {
        format!("https://{trimmed}")
    } else {
        return None;
    };

    let parsed = url::Url::parse(&candidate).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    match parsed.host_str() {
        Some(host) if !host.is_empty() => Some(parsed.to_string()),
        _ => None,
    }
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
    fn body_limit_is_enforced_across_chunks() {
        let mut bytes = vec![b'x'; MAX_WEB_BODY_BYTES - 2];
        append_bounded(&mut bytes, b"ab").unwrap();
        assert_eq!(bytes.len(), MAX_WEB_BODY_BYTES);
        assert!(append_bounded(&mut bytes, b"c").is_err());
        assert_eq!(bytes.len(), MAX_WEB_BODY_BYTES);
        assert!(append_bounded(&mut Vec::new(), &vec![0; MAX_WEB_BODY_BYTES + 1]).is_err());
    }

    #[tokio::test]
    async fn oversized_http_response_is_rejected_without_reading_body() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.read(&mut [0; 2048]);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_WEB_BODY_BYTES + 1
            );
        });
        let response = reqwest::get(format!("http://{address}/")).await.unwrap();
        assert!(read_bounded(response).await.is_err());
        server.join().unwrap();
    }

    #[test]
    fn wikipedia_fallback_parses_scoped_results_and_rejects_invalid_responses() {
        let body = r#"{"query":{"search":[{"pageid":42,"title":"A & B","snippet":"A <span class=\"searchmatch\">sample</span> &amp; text"},{"pageid":"bad","title":"ignored"}]}}"#;
        let hits = parse_wikipedia_results(body).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://en.wikipedia.org/?curid=42");
        assert_eq!(hits[0].title, "A & B");
        assert!(hits[0].snippet.contains("sample"));
        assert!(!hits[0].snippet.contains("<span"));
        assert!(parse_wikipedia_results("not json").is_err());
        assert!(parse_wikipedia_results(r#"{"error":{"code":"bad"}}"#).is_err());
        assert!(parse_wikipedia_results(r#"{"query":{"search":[]}}"#)
            .unwrap()
            .is_empty());
    }

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
        assert_eq!(normalise_url("javascript://alert(1)"), None);
        assert_eq!(
            normalise_url("example.com:8080/path"),
            Some("https://example.com:8080/path".to_string())
        );
        assert_eq!(
            normalise_url("https://example.com/a?b=c"),
            Some("https://example.com/a?b=c".to_string())
        );
    }

    #[test]
    fn query_encoding_handles_spaces_and_symbols() {
        assert_eq!(urlencode("sydney weather"), "sydney+weather");
        assert_eq!(urlencode("a&b"), "a%26b");
    }
}
