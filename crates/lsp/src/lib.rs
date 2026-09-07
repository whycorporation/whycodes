pub mod client;
pub mod config;
pub mod detect;
pub mod error;
pub mod tool;
pub mod types;

pub use config::{LspServerSpec, LspSettings, ResolvedServer};
pub use detect::{file_uri, path_from_file_uri};
pub use error::{LspError, Result};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
