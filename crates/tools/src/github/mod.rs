pub mod api;
pub mod issue;
pub mod pr;

#[cfg(test)]
mod tests {
    #[test]
    fn mod_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
