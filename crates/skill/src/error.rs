use thiserror::Error;

/// Skill / plugin-config load failures.
#[derive(Debug, Error)]
pub enum SkillError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl SkillError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}

pub type Result<T> = std::result::Result<T, SkillError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_and_io_display() {
        let msg = SkillError::msg("bad skill");
        assert_eq!(msg.to_string(), "bad skill");
        let io = SkillError::from(std::io::Error::other("disk"));
        assert!(io.to_string().contains("disk"));
    }
}
