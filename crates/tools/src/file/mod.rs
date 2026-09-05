pub mod apply_patch;
pub(crate) mod atomic;
pub mod edit;
pub mod external_directory;
pub mod glob;
pub mod grep;
pub mod internal;
pub mod list;
pub mod paths;
pub mod read;
pub mod repomap;
pub mod truncate;
pub mod truncation_dir;
pub mod write;

#[cfg(test)]
mod tests {
    #[test]
    fn mod_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
