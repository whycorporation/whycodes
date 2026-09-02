pub mod background;
pub mod checkpoint;
pub mod code_mode;
pub mod memory;
pub mod panel;
pub mod plan;
pub mod question;
pub mod schedule;
pub mod skill;
pub mod swarm;
pub mod swarm_message;
pub mod task;
pub mod todo_read;
pub mod todo_write;
pub mod tool_search;
pub mod worktree;

#[cfg(test)]
mod tests {
    #[test]
    fn mod_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
