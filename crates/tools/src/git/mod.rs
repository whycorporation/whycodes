pub mod blame;
pub mod commit;
pub mod diff;
pub mod log;
pub mod status;

#[cfg(test)]
mod tests {
    #[test]
    fn mod_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
