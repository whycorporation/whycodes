use super::*;

#[test]
fn cancel_reaches_only_a_registered_worker() {
    let cancels = WorkerCancels::default();
    assert!(!cancels.cancel("task-1"), "nothing runs yet");
    assert!(!cancels.is_cancelled("task-1"));

    cancels.register("task-1");
    assert!(!cancels.is_cancelled("task-1"));
    assert!(
        cancels.clone().cancel("task-1"),
        "clones share the registry"
    );
    assert!(cancels.is_cancelled("task-1"));

    cancels.finish("task-1");
    assert!(!cancels.is_cancelled("task-1"));
    assert!(
        !cancels.cancel("task-1"),
        "a finished worker is not cancelled"
    );

    cancels.register("task-1");
    assert!(!cancels.is_cancelled("task-1"), "a reused id starts clean");
}
