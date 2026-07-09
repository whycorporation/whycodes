use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct WebSearchTool;

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
        "Search the web using a search engine and return results. Requires SERPAPI_API_KEY environment variable."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
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
                urlencoding(&query),
                api_key,
                num_results
            );

            match reqwest::get(&url).await {
                Ok(response) => match response.json::<serde_json::Value>().await {
                    Ok(data) => {
                        let mut results = String::new();
                        if let Some(organic) = data["organic_results"].as_array() {
                            for (i, result) in organic.iter().enumerate() {
                                let title = result["title"].as_str().unwrap_or("No title");
                                let link = result["link"].as_str().unwrap_or("No link");
                                let snippet = result["snippet"].as_str().unwrap_or("");
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
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding(query)
        );

        match reqwest::get(&url).await {
            Ok(response) => match response.text().await {
                Ok(html) => {
                    // Simple extraction of result snippets
                    let mut results: Vec<String> = Vec::new();
                    for line in html.lines() {
                        if line.contains("result__snippet") {
                            if let Some(start) = line.find('>') {
                                if let Some(end) = line.rfind('<') {
                                    if start + 1 < end {
                                        let snippet = &line[start + 1..end];
                                        results.push(snippet.trim().to_string());
                                    }
                                }
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

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}
