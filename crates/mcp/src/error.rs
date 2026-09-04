use thiserror::Error;

/// MCP client / transport / stdio-server failures.
#[derive(Debug, Error)]
pub enum McpError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

impl McpError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}

pub type Result<T> = std::result::Result<T, McpError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_io_json_display() {
        assert_eq!(McpError::msg("no stdin").to_string(), "no stdin");
        let io = McpError::from(std::io::Error::other("pipe"));
        assert!(io.to_string().contains("pipe"));
        let json = McpError::from(serde_json::from_str::<u8>("x").unwrap_err());
        assert!(!json.to_string().is_empty());
    }

    #[tokio::test]
    async fn http_error_conversion() {
        let err = reqwest::Client::new()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err();
        let wrapped = McpError::from(err);
        assert!(!wrapped.to_string().is_empty());
    }
}
