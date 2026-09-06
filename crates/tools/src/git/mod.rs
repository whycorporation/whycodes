pub mod blame;
pub mod commit;
pub mod diff;
pub mod log;
pub mod status;

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "mod_tests.rs"]
mod skipped_tests;

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "mod_inline_tests.rs"]
mod tests;
