use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

/// Arguments for the WebFetch tool
#[derive(Debug, Deserialize)]
pub struct WebFetchArgs {
    /// URL to fetch
    pub url: String,
    /// Optional size limit in KB (default: 100KB)
    pub size_limit_kb: Option<usize>,
}

/// Tool to fetch content from a URL
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct WebFetch;

impl Tool for WebFetch {
    const NAME: &'static str = "web_fetch";
    type Error = ToolError;
    type Args = WebFetchArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Fetch content from a URL. Automatically converts HTML to readable text. Returns content with metadata (status, content type, final URL).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch (must start with http:// or https://)"
                    },
                    "size_limit_kb": {
                        "type": "integer",
                        "description": "Optional size limit in KB (default: 100KB). Maximum allowed is 500KB."
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Validate and parse URL
        let parsed_url = url::Url::parse(&args.url)
            .map_err(|e| ToolError::invalid_url(format!("Invalid URL: {}", e)))?;

        // Security check: reject file:// and local URLs
        if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
            return Err(ToolError::invalid_url(format!(
                "Only HTTP and HTTPS URLs are allowed, got: {}",
                parsed_url.scheme()
            )));
        }

        // Check for localhost/private IPs (basic SSRF protection)
        if let Some(host) = parsed_url.host_str()
            && (host == "localhost"
                || host == "127.0.0.1"
                || host.starts_with("192.168.")
                || host.starts_with("10.")
                || host.starts_with("172.16.")
                || host.starts_with("172.17.")
                || host.starts_with("172.18.")
                || host.starts_with("172.19.")
                || host.starts_with("172.20.")
                || host.starts_with("172.21.")
                || host.starts_with("172.22.")
                || host.starts_with("172.23.")
                || host.starts_with("172.24.")
                || host.starts_with("172.25.")
                || host.starts_with("172.26.")
                || host.starts_with("172.27.")
                || host.starts_with("172.28.")
                || host.starts_with("172.29.")
                || host.starts_with("172.30.")
                || host.starts_with("172.31.")) {
                return Err(ToolError::invalid_url(
                    "Cannot fetch from localhost or private IP addresses".to_string()
                ));
            }

        // Set size limit (default 100KB, max 500KB)
        let size_limit = args.size_limit_kb.unwrap_or(100).min(500) * 1024;

        // Build HTTP client
        let client = reqwest::Client::builder()
            .user_agent("agent-t/1.0 (Terminal AI Agent)")
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| ToolError::network_error(format!("Failed to create HTTP client: {}", e)))?;

        // Fetch the URL
        let response = client
            .get(parsed_url.as_str())
            .send()
            .await
            .map_err(|e| ToolError::network_error(format!("Failed to fetch URL: {}", e)))?;

        // Get metadata
        let status = response.status();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        // Check status code
        if !status.is_success() {
            return Err(ToolError::http_error(format!(
                "HTTP {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            )));
        }

        // Read response body with size limit
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ToolError::network_error(format!("Failed to read response: {}", e)))?;

        if bytes.len() > size_limit {
            return Err(ToolError::http_error(format!(
                "Response size ({} bytes) exceeds limit ({} bytes)",
                bytes.len(),
                size_limit
            )));
        }

        // Convert to string
        let content = String::from_utf8_lossy(&bytes).to_string();

        // Process content based on type
        let processed_content = if content_type.contains("html") {
            // Convert HTML to readable text
            html2text::from_read(content.as_bytes(), 80)
        } else {
            content
        };

        // Format output with metadata
        let output = format!(
            "Status: {}\nContent-Type: {}\nFinal URL: {}\nSize: {} bytes\n\n{}\n",
            status.as_u16(),
            content_type,
            final_url,
            bytes.len(),
            processed_content.trim()
        );

        Ok(output)
    }
}
