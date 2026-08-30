pub mod error;
pub mod plugin;
pub mod registry;
pub mod skill;

pub use error::{Result, SkillError};
pub use plugin::{Plugin, PluginConfig, PluginContext};
pub use registry::{PluginRegistry, SkillRegistry};
pub use skill::Skill;
