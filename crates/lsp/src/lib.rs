pub mod client;
pub mod error;
pub mod tool;
pub mod types;

pub use error::{LspError, Result};

#[cfg(test)]
mod tests {
    #[test]
    fn lib_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
