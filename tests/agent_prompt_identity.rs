//! An agent must be able to tell which machine it is on.
//!
//! Its name is not an identity: several devices in a Circle may each configure
//! an agent called `suzent`, and the roster shows them all. Without knowing
//! which one it is, an agent cannot say which machine it runs on, cannot
//! address a specific sibling, and a user ends up hand-routing with "mention
//! the one on <device>".

use enoxian::agent::context::build_prompt;
use enoxian::config::JoinPolicy;
use enoxian::control::{MemberEntry, MemberRole, MEMBER_LIST_KEY};
use enoxian::mls;
use enoxian::state::AppState;
use yrs::{Any, Map, Transact, WriteTxn};

const ME: &str = "peer-macbook";
const OTHER: &str = "peer-jessair";

fn state(dir: &std::path::Path) -> AppState {
    AppState::new(
        "circle-1".into(),
        "group".into(),
        dir.to_path_buf(),
        dir.to_path_buf(),
        String::new(),
        "suzy-local".into(),
        1,
        ME.into(),
        JoinPolicy::Manual,
        "suzy".into(),
        mls::new_mls_state(mls::MlsIdentity::generate(ME).unwrap(), None),
    )
}

fn member(peer_id: &str, device: &str, agents: &[&str]) -> MemberEntry {
    MemberEntry {
        peer_id: peer_id.into(),
        owner: "suzy".into(),
        agent_id: format!("suzy-{device}"),
        device_label: device.into(),
        agents: agents.iter().map(|s| s.to_string()).collect(),
        ambient_agents: Vec::new(),
        role: MemberRole::Admin,
        added_at: chrono::Utc::now(),
        signature: String::new(),
    }
}

/// Two machines, each running an agent called `suzent`.
fn two_devices(state: &AppState) {
    let mut txn = state.control.try_transact_mut().unwrap();
    let map = txn.get_or_insert_map(MEMBER_LIST_KEY);
    for (peer, device) in [(ME, "macbook-pro"), (OTHER, "jessair")] {
        let entry = serde_json::to_string(&member(peer, device, &["suzent"])).unwrap();
        map.insert(&mut txn, peer, Any::String(entry.as_str().into()));
    }
}

fn fresh_prompt(state: &AppState) -> String {
    build_prompt(state, "suzent", "suzy", "do the thing", None, "m1")
}

#[test]
fn the_brief_says_which_device_the_agent_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    two_devices(&state);

    let prompt = fresh_prompt(&state);
    assert!(
        prompt.contains("macbook-pro"),
        "the agent must be told its own device:\n{prompt}"
    );
    assert!(
        prompt.contains("@suzy/macbook-pro/suzent"),
        "and its exact address, so it can be named unambiguously:\n{prompt}"
    );
}

#[test]
fn identity_is_resolved_by_peer_not_by_agent_name() {
    // Both devices list an agent named `suzent`, so a name match would pick
    // whichever came first. Only peer_id distinguishes them.
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    two_devices(&state);

    let prompt = fresh_prompt(&state);
    assert!(
        !prompt.contains("@suzy/jessair/suzent"),
        "the agent must not be told it is the *other* device's agent:\n{prompt}"
    );
    // The sibling is still visible in the roster — it just isn't "you".
    assert!(
        prompt.contains("jessair"),
        "the roster still lists the other device"
    );
}

#[test]
fn the_roster_marks_which_entry_is_this_machine() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    two_devices(&state);

    let prompt = fresh_prompt(&state);
    let marked: Vec<&str> = prompt
        .lines()
        .flat_map(|l| l.split(", "))
        .filter(|part| part.contains("← you are here"))
        .collect();
    assert_eq!(
        marked.len(),
        1,
        "exactly one entry is this machine: {marked:?}"
    );
    assert!(
        marked[0].contains("macbook-pro"),
        "and it is the right one: {marked:?}"
    );
}

#[test]
fn the_brief_explains_that_handing_work_over_is_possible_and_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    two_devices(&state);

    let prompt = fresh_prompt(&state);
    assert!(
        prompt.contains("hand it over by mentioning it"),
        "delegation shipped; an agent that is not told about it will never use it:\n{prompt}"
    );
    assert!(
        prompt.contains("that device's own decision"),
        "and it must not assume a hand-off always runs"
    );
}

#[test]
fn an_empty_roster_degrades_quietly_rather_than_guessing() {
    // A Circle whose member list has not synced yet: say nothing about
    // identity rather than assert something wrong.
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());

    let prompt = fresh_prompt(&state);
    assert!(!prompt.contains("You are running on device"));
    assert!(!prompt.contains("← you are here"));
    // The rest of the brief still arrives.
    assert!(prompt.contains("an agent participating in an enoxian circle"));
    assert!(prompt.ends_with("do the thing"));
}
