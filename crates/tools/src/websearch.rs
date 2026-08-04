use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use super::webfetch::html_to_text;
use whycode_core::types::ToolResult;

pub struct WebSearchTool;

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "websearch"
    }

    fn description(&self) -> &str {
        "Search the web and return result snippets. Requires SERPAPI_API_KEY for best results \
         (falls back to DuckDuckGo HTML). \
         For 'latest version' questions: do not pin the query to a past calendar year \
         (e.g. avoid '2024'/'2025'); search 'Nuxt latest release' or fetch canonical APIs \
         via webfetch (npm registry, GitHub Releases, official docs)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query. Prefer current phrasing without a past year for 'latest' lookups."
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let query = args["query"].as_str().unwrap_or("");
        let num_results = args["num_results"].as_u64().unwrap_or(10);

        if query.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "Query is required.".to_string(),
                is_error: true,
            };
        }

        // Try SerpAPI first
        if let Ok(api_key) = std::env::var("SERPAPI_API_KEY") {
            let url = format!(
                "https://serpapi.com/search?q={}&api_key={}&num={}&engine=google",
                urlencoding(query),
                api_key,
                num_results
            );

            match reqwest::get(&url).await {
                Ok(response) => match response.json::<serde_json::Value>().await {
                    Ok(data) => {
                        let mut results = String::new();
                        if let Some(organic) = data["organic_results"].as_array() {
                            for (i, result) in organic.iter().enumerate() {
                                let title =
                                    strip_markup(result["title"].as_str().unwrap_or("No title"));
                                let link = result["link"].as_str().unwrap_or("No link");
                                let snippet =
                                    strip_markup(result["snippet"].as_str().unwrap_or(""));
                                results.push_str(&format!(
                                    "{}. {}\n   {}\n   {}\n\n",
                                    i + 1,
                                    title,
                                    link,
                                    snippet
                                ));
                            }
                        }

                        return ToolResult {
                            tool_call_id: String::new(),
                            content: if results.is_empty() {
                                "No results found.".to_string()
                            } else {
                                results
                            },
                            is_error: false,
                        };
                    }
                    Err(e) => {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: format!("Error parsing search results: {}", e),
                            is_error: true,
                        };
                    }
                },
                Err(e) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error performing search: {}", e),
                        is_error: true,
                    };
                }
            }
        }

        // Fallback: try DuckDuckGo HTML search
        let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding(query));

        match reqwest::get(&url).await {
            Ok(response) => match response.text().await {
                Ok(html) => {
                    // Simple extraction of result snippets
                    let mut results: Vec<String> = Vec::new();
                    for line in html.lines() {
                        if line.contains("result__snippet")
                            && let Some(start) = line.find('>')
                            && let Some(end) = line.rfind('<')
                            && start + 1 < end
                        {
                            let snippet = strip_markup(&line[start + 1..end]);
                            if !snippet.is_empty() {
                                results.push(snippet);
                            }
                        }
                    }

                    results.truncate(num_results as usize);

                    ToolResult {
                        tool_call_id: String::new(),
                        content: if results.is_empty() {
                            "No results found. Set SERPAPI_API_KEY for better results.".to_string()
                        } else {
                            results
                                .iter()
                                .enumerate()
                                .map(|(i, s)| format!("{}. {}", i + 1, s))
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        },
                        is_error: false,
                    }
                }
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error reading response: {}", e),
                    is_error: true,
                },
            },
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error performing search: {}", e),
                is_error: true,
            },
        }
    }
}

/// Strip residual HTML tags/entities from SERP snippets.
fn strip_markup(s: &str) -> String {
    if s.contains('<') || s.contains('&') {
        html_to_text(s)
    } else {
        s.trim().to_string()
    }
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_markup_removes_nested_bold() {
        let s = "Preparing for <b>Nuxt</b> 5";
        let out = strip_markup(s);
        assert!(!out.contains("<b>"));
        assert!(out.contains("Nuxt"));
        assert!(out.contains("5"));
    }
}
