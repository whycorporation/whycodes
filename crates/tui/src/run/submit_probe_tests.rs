//! Submit / turn-end phase timings against a real home and project.
//!
//! Ignored by default: it reads a copy of a real `~/.whycodes` (config, DB,
//! memory) so the numbers reflect a lived-in install, not an empty temp home.
//!
//! ```text
//! WHYCODES_PROBE_HOME=<copy of ~/.whycodes> WHYCODES_PROBE_PROJECT=<repo> \
//!   cargo test -p whycodes-tui --release --lib submit_phase_probe -- --ignored --nocapture
//! ```
//!
//! Point it at a **copy**: memory recall bumps recall counts and the persist
//! phase rewrites the largest session's rows.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::*;

fn median(mut v: Vec<Duration>) -> f64 {
    v.sort();
    v[v.len() / 2].as_secs_f64() * 1e3
}

#[test]
#[ignore = "timing probe; needs WHYCODES_PROBE_HOME (a copy) and WHYCODES_PROBE_PROJECT"]
fn submit_phase_probe() {
    let (Some(home), Some(project)) = (
        std::env::var_os("WHYCODES_PROBE_HOME"),
        std::env::var_os("WHYCODES_PROBE_PROJECT"),
    ) else {
        println!("submit_phase_probe: set WHYCODES_PROBE_HOME and WHYCODES_PROBE_PROJECT");
        return;
    };
    unsafe { std::env::set_var("WHYCODES_HOME", &home) };
    let project = PathBuf::from(project);
    let config = Config::load().expect("config");
    let db_path = Config::data_dir().expect("data dir").join("whycodes.db");
    let db = whycodes_storage::db::Database::open(&db_path.to_string_lossy()).expect("db");

    // Largest saved session stands in for a long-running conversation.
    let counts = db.message_counts_by_session().expect("counts");
    let (sid, n) = counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .expect("at least one saved session");
    let mut session = Session::load_from_db(&db, &sid)
        .expect("load")
        .expect("session row");
    session.project_path = project.clone();
    println!("submit_phase_probe: session {sid} with {n} messages");

    let agent = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet::default(),
        model: None,
        system_prompt: None,
        temperature: None,
        top_p: None,
    })
    .with_config(&config);
    let prompt = "why does the stream frame cost grow with reply length in the chat renderer?";
    let mut history = whycodes_session::SessionHistory::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (mut ui, mut prep, mut persist) = (vec![], vec![], vec![]);
    for _ in 0..5 {
        // What still runs on the event loop at submit (record_user_turn).
        let t = Instant::now();
        let _ = expand_at_files(prompt, &project);
        let stash = history.push_before_turn(&session.messages, &project);
        ui.push(t.elapsed());

        // What moved into the turn task, before the first LLM step.
        let t = Instant::now();
        rt.block_on(prepare_turn(
            &agent,
            &mut session,
            stash,
            &project,
            &config,
            prompt,
        ));
        prep.push(t.elapsed());

        let t = Instant::now();
        session.save_to_db(&db).expect("persist");
        persist.push(t.elapsed());
    }
    let (mut mem, mut git) = (vec![], vec![]);
    for _ in 0..5 {
        let t = Instant::now();
        refresh_session_memory(&mut session, &agent, &project, &config, Some(prompt));
        mem.push(t.elapsed());
        let t = Instant::now();
        let _ = history.push_before_turn(&session.messages, &project).wait();
        git.push(t.elapsed());
    }
    let (mut agents, mut index, mut recall, mut open) = (vec![], vec![], vec![], vec![]);
    let data_dir = Config::data_dir().expect("data");
    let settings = super::persist::memory_settings(&config);
    for _ in 0..5 {
        let t = Instant::now();
        let base = Agent::with_agents_md("base", &project);
        agents.push(t.elapsed());
        let t = Instant::now();
        let _ = whycodes_memory::apply_memory_prompt(&base, &project, &data_dir, &settings, None);
        index.push(t.elapsed());
        let t = Instant::now();
        let _ = whycodes_memory::recall_turn_context(&project, &data_dir, &settings, prompt);
        recall.push(t.elapsed());
        let t = Instant::now();
        let _ = whycodes_memory::MemoryService::open(&project, &data_dir, settings.clone());
        open.push(t.elapsed());
    }
    println!("  with_agents_md         {:8.2} ms", median(agents));
    println!("  index (open + MEMORY)  {:8.2} ms", median(index));
    println!("  recall (open + FTS)    {:8.2} ms", median(recall));
    println!("  MemoryService::open    {:8.2} ms", median(open));
    println!("  memory refresh alone   {:8.2} ms", median(mem));
    println!("  undo stash alone       {:8.2} ms", median(git));
    println!("  event loop at submit   {:8.2} ms", median(ui));
    println!("  prepare_turn (task)    {:8.2} ms", median(prep));
    println!("  persist (save_to_db)   {:8.2} ms", median(persist));
}
