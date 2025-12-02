use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

/// Arguments for the WebSearch tool
#[derive(Debug, Deserialize)]
pub struct WebSearchArgs {
    /// Search query
    pub query: String,
    /// Number of results to return (default: 5, max: 10)
    pub num_results: Option<usize>,
}

/// Tool to search the web
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct WebSearch;

impl Tool for WebSearch {
    const NAME: &'static str = "web_search";
    type Error = ToolError;
    type Args = WebSearchArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search the web using DuckDuckGo. Returns a list of search results with title, URL, and snippet for each result.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "num_results": {
                        "type": "integer",
                        "description": "Number of results to return (default: 5, max: 10)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.query.trim().is_empty() {
            return Err(ToolError::invalid_arguments("Search query cannot be empty"));
        }

        let num_results = args.num_results.unwrap_or(5).min(10);

        // Build HTTP client
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| ToolError::network_error(format!("Failed to create HTTP client: {}", e)))?;

        // Use DuckDuckGo HTML interface
        let search_url = format!("https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&args.query));

        // Fetch search results
        let response = client
            .get(&search_url)
            .send()
            .await
            .map_err(|e| ToolError::network_error(format!("Failed to fetch search results: {}", e)))?;

        if !response.status().is_success() {
            return Err(ToolError::http_error(format!(
                "Search request failed with status: {}",
                response.status()
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| ToolError::network_error(format!("Failed to read response: {}", e)))?;

        // Parse search results
        let results = parse_duckduckgo_results(&html, num_results);

        if results.is_empty() {
            return Ok("No search results found.".to_string());
        }

        // Format output
        let mut output = format!("Found {} search results for \"{}\":\n\n", results.len(), args.query);

        for (idx, result) in results.iter().enumerate() {
            output.push_str(&format!(
                "{}. {}\n   URL: {}\n   {}\n\n",
                idx + 1,
                result.title,
                result.url,
                result.snippet
            ));
        }

        Ok(output)
    }
}

/// Represents a search result
#[derive(Debug)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Parse DuckDuckGo HTML results
fn parse_duckduckgo_results(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Find all result divs (class="result")
    let mut pos = 0;
    while results.len() < limit {
        // Find next result div
        let Some(result_start) = html[pos..].find("class=\"result ") else {
            break;
        };
        pos += result_start;

        // Find the end of this result div
        let result_html = &html[pos..];
        let Some(result_end) = result_html.find("</div>") else {
            break;
        };
        let result_section = &result_html[..result_end];

        // Extract title and URL
        if let Some(title_data) = extract_title_and_url(result_section) {
            // Extract snippet
            let snippet = extract_snippet(result_section);

            results.push(SearchResult {
                title: title_data.0,
                url: title_data.1,
                snippet,
            });
        }

        pos += result_end + 6; // +6 for "</div>"
    }

    results
}

/// Extract title and URL from a result section
fn extract_title_and_url(html: &str) -> Option<(String, String)> {
    // Find the title link (class="result__a")
    let link_start = html.find("class=\"result__a\"")?;
    let link_section = &html[link_start..];

    // Extract href
    let href_start = link_section.find("href=\"")? + 6;
    let href_end = link_section[href_start..].find("\"")?;
    let mut url = link_section[href_start..href_start + href_end].to_string();

    // DuckDuckGo uses redirect URLs, extract the actual URL
    if url.starts_with("//duckduckgo.com/l/?uddg=")
        && let Some(uddg_start) = url.find("uddg=") {
            let encoded_url = &url[uddg_start + 5..];
            if let Some(amp_pos) = encoded_url.find('&') {
                url = urlencoding::decode(&encoded_url[..amp_pos])
                    .unwrap_or_default()
                    .to_string();
            } else {
                url = urlencoding::decode(encoded_url)
                    .unwrap_or_default()
                    .to_string();
            }
        }

    // Extract title text
    let title_start = link_section.find('>')? + 1;
    let title_end = link_section[title_start..].find("</a>")?;
    let title = decode_html(&link_section[title_start..title_start + title_end]);

    Some((title, url))
}

/// Extract snippet from a result section
fn extract_snippet(html: &str) -> String {
    // Find the snippet div (class="result__snippet")
    if let Some(snippet_start) = html.find("class=\"result__snippet\">") {
        let snippet_section = &html[snippet_start + 24..]; // +24 for the class string
        if let Some(snippet_end) = snippet_section.find("</") {
            let snippet = decode_html(&snippet_section[..snippet_end]);
            return snippet;
        }
    }
    String::new()
}

/// Decode basic HTML entities
fn decode_html(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("<b>", "")
        .replace("</b>", "")
        .replace("<em>", "")
        .replace("</em>", "")
        .trim()
        .to_string()
}

// Add urlencoding dependency helper
mod urlencoding {
    pub fn encode(input: &str) -> String {
        input
            .chars()
            .map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
                ' ' => "+".to_string(),
                _ => format!("%{:02X}", c as u8),
            })
            .collect()
    }

    pub fn decode(input: &str) -> Result<String, String> {
        let mut result = String::new();
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '+' => result.push(' '),
                '%' => {
                    let hex: String = chars.by_ref().take(2).collect();
                    if hex.len() == 2 {
                        if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                            result.push(byte as char);
                        } else {
                            return Err("Invalid hex encoding".to_string());
                        }
                    } else {
                        return Err("Incomplete percent encoding".to_string());
                    }
                }
                _ => result.push(c),
            }
        }

        Ok(result)
    }
}
