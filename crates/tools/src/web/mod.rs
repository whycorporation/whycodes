pub mod browser;
pub mod fetch;
pub mod mcp_search;
pub mod search;

#[cfg(test)]
mod tests {
    #[test]
    fn mod_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
