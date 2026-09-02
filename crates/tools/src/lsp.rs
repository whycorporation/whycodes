// Thin re-export of the LspTool from whycodes-lsp so the tools crate
// can register it alongside all other built-in tools.
pub use whycodes_lsp::tool::LspTool;

#[cfg(test)]
mod tests {
    #[test]
    fn lsp_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
