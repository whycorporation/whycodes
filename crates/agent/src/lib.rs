pub mod agent;
pub mod background;
#[cfg(test)]
mod behavior_eval;
pub mod context_files;
pub mod events;
pub mod intent;
pub mod magic_keywords;
pub mod mcp_load;
pub mod memory_retain;
pub mod notify;
pub mod permission;
pub mod question;
pub mod routing;
pub mod speculative_read;
pub mod subagent;
pub mod swarm;
pub mod swarm_worktree;
#[cfg(test)]
mod tests;
pub mod thinking_acc;
pub mod title;
mod tool_policy;
pub mod tool_stream;

pub use agent::{Agent, memory_settings_from_config};
pub use background::{BackgroundRegistry, JobSnapshot, JobStatus};
pub use events::{
    CancelFlag, TurnEvent, TurnOpts, is_cancelled, new_cancel_flag, request_cancel,
    wait_until_cancelled,
};
pub use intent::{
    IntentAssessment, IntentGuidanceMode, IntentNotice, IntentNoticeKind, ToolAuthDecision,
    UserIntent, authorize_tool, badge_label, classify_user_intent, intent_notice,
    is_read_only_shell,
};
pub use permission::{
    AutoApprovePrompter, AutoDenyPrompter, ChannelPermissionPrompter, PermissionPrompter,
    PermissionRequest, StdinPrompter, default_prompter,
};
pub use question::{
    AutoAnswerPrompter, ChannelQuestionPrompter, QuestionError, QuestionPrompter, QuestionRequest,
    StdinQuestionPrompter, default_question_prompter, run_question_tool,
};
pub use routing::{
    resolve_agent_model, resolve_override, resolve_turn_model, resolve_worker_model,
};
pub use title::{generate_title, is_trivial_title_seed, resolve_title_model, should_refine_title};
