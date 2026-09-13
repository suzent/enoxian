//! Explicitly invoked provider smoke. Default tests never contact a model.
use enoxian::agent::{
    config::{AgentConfig, Driver},
    driver::{launch, Initiator, LaunchRequest},
};

#[tokio::test]
#[ignore = "uses the configured local Claude adapter for two short isolated turns"]
async fn configured_acp_retains_one_conversation_across_runs() {
    let cfg = AgentConfig::load();
    let mut cmd = cfg
        .resolve("claude")
        .expect("configure claude first")
        .clone();
    assert!(matches!(cmd.driver, Driver::Acp));
    cmd.working_dir = None;
    let workspace = tempfile::tempdir().unwrap();
    let records = tempfile::tempdir().unwrap();
    for task in [
        "Remember the exact marker ENOX_CONTINUITY_4729 for this conversation. Reply only OK. Do not use tools or modify files.",
        "What exact marker did I ask you to remember in the previous turn? Reply only with that marker. Do not use tools or modify files.",
    ] {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(90), launch(LaunchRequest {
            run_id: None, trigger_id: None, coordination: None,
            agent_name: "claude", cmd: &cmd, task, workspace: workspace.path(),
            base_snapshot: "", circle_id: "isolated-smoke", circle_dir: records.path(),
            actor_token: None, initiator: Initiator::Local, relay_path: vec![], resume: None,
        })).await.expect("provider timeout").unwrap();
        assert!(outcome.acp_session_id.is_some());
        if task.starts_with("What") {
            assert!(outcome.reply.as_deref().unwrap_or("").contains("ENOX_CONTINUITY_4729"));
        }
    }
    let runs = enoxian::proposal::runs::list(records.path()).unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|r| !r.session.is_open()));
}
