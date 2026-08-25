use crate::control::{
    ChatActivity, ChatMessage, CircleEvent, Presence, Task, TaskStatus, CHAT_ACTIVITY_KEY,
    CHAT_KEY, MEMBER_LIST_KEY, MLS_REMOVED_KEY, PRESENCE_KEY, TASKS_KEY,
};
use dashmap::DashMap;
use libp2p::{multiaddr::Protocol, swarm::ConnectionId, Multiaddr};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use yrs::{Any, Doc, Map, Observable, Out, Transact};

pub const EVENT_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionKind {
    Lan,
    Tailscale,
    Public,
    Relay,
}

impl ConnectionKind {
    fn rank(self) -> u8 {
        match self {
            Self::Lan => 0,
            Self::Tailscale => 1,
            Self::Public => 2,
            Self::Relay => 3,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PeerConnection {
    pub kind: ConnectionKind,
    pub address: String,
}

/// Shared state — Clone is cheap (all fields are Arc).
#[derive(Clone)]
pub struct AppState {
    pub circle_id: String,
    pub circle_name: String,
    pub workspace: PathBuf,
    /// ~/.enoxian/circles/<circle_id>/ — used for session and peer records.
    pub circle_dir: PathBuf,
    pub admin_pubkey_hex: String,
    pub agent_id: String,
    /// Monotonically increasing counter, incremented on every daemon start.
    pub session_id: u64,
    /// This node's libp2p peer ID.
    pub peer_id: String,
    /// Externally-confirmed TCP multiaddrs for this node (populated by Identify / ExternalAddrConfirmed).
    /// Used by `enox invite` to auto-embed a connectable peer address.
    pub p2p_external_addrs: Arc<RwLock<Vec<String>>>,
    /// Local listen multiaddrs (non-loopback, non-unspecified, non-circuit).
    /// On a VPS with a public IP these include the real address immediately at startup,
    /// before any peer connects to confirm via Identify. Used as fallback for `enox invite`.
    pub p2p_listen_addrs: Arc<RwLock<Vec<String>>>,
    /// Connections observed by this daemon. This stays local because each peer
    /// can see a different route to the same member.
    peer_connections: Arc<RwLock<HashMap<String, HashMap<ConnectionId, PeerConnection>>>>,
    /// File docs. Key = relative path with forward slashes.
    pub docs: Arc<DashMap<String, Arc<Doc>>>,
    /// __control__ coordination document
    pub control: Arc<Doc>,
    /// Per-doc raw v1 update bytes broadcast (for WS clients and local subscribers)
    pub doc_updates: Arc<DashMap<String, broadcast::Sender<Vec<u8>>>>,
    /// Per-doc awareness bytes broadcast — relays cursor/presence updates between WS clients.
    pub awareness_updates: Arc<DashMap<String, broadcast::Sender<Vec<u8>>>>,
    /// Global broadcast: (rel_path, raw_v1_update). Used by P2P sync to forward local updates.
    pub all_updates: broadcast::Sender<(String, Vec<u8>)>,
    /// Global broadcast: (rel_path, y-protocols awareness message). Used by P2P sync
    /// to forward ephemeral cursor/presence updates without storing them in CRDT docs.
    pub all_awareness_updates: broadcast::Sender<(String, Vec<u8>)>,
    /// Global broadcast: rel_path deletions. Used by P2P sync to propagate file
    /// removals, which cannot be represented by deleting a Yjs text doc update.
    pub all_deletes: broadcast::Sender<String>,
    /// SSE event stream
    pub events: broadcast::Sender<CircleEvent>,
    /// Paths written to disk by interactive surfaces (browser WS edits, P2P
    /// CRDT sync, UI file operations). The proposal engine folds these into
    /// its baseline without creating proposals — they are already
    /// user-visible live edits, not reviewable agent changes.
    /// Payload: (rel_path, author_label). author_label is None for local writes;
    /// for P2P writes it carries the remote peer's device_label so the proposal
    /// is attributed to the correct device instead of always stamping the local one.
    pub interactive_writes: broadcast::Sender<(String, Option<String>)>,
    /// (path, expected blob hash) written by the proposal review API when a
    /// reject/revert restores files (None = path deleted). The engine folds
    /// matching changes into its baseline silently so review decisions never
    /// spawn follow-up proposals.
    pub review_writes: broadcast::Sender<(String, Option<String>)>,
    /// Per-path flag: set to true before flush_to_disk writes, cleared by watcher on receipt.
    /// Shared between the file watcher and flush_to_disk so they operate on the same flag.
    pub self_write_flags: Arc<DashMap<String, Arc<AtomicBool>>>,
    pub join_policy: crate::config::JoinPolicy,
    pub owner: String,
    pub mls: crate::mls::SharedMlsState,
    /// Recent P2P connection failures (unix_ts, message). Surfaced by the status
    /// API so silent handshake failures (e.g. PSK mismatch) are diagnosable.
    pub recent_conn_errors: Arc<RwLock<std::collections::VecDeque<(i64, String)>>>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        circle_id: String,
        circle_name: String,
        workspace: PathBuf,
        circle_dir: PathBuf,
        admin_pubkey_hex: String,
        agent_id: String,
        session_id: u64,
        peer_id: String,
        join_policy: crate::config::JoinPolicy,
        owner: String,
        mls: crate::mls::SharedMlsState,
    ) -> Self {
        let (events_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (interactive_writes_tx, _): (broadcast::Sender<(String, Option<String>)>, _) =
            broadcast::channel(EVENT_CAPACITY);
        let (review_writes_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (all_updates_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (all_awareness_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (all_deletes_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let control = Arc::new(Doc::new());

        // Forward control doc updates to P2P peers (skip updates that arrived from peers).
        let all_tx = all_updates_tx.clone();
        let sub = control
            .observe_update_v1(move |txn, event| {
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                if !is_p2p {
                    let _ = all_tx.send(("__control__".to_string(), event.update.clone()));
                }
            })
            .expect("observe control doc failed");
        std::mem::forget(sub);

        // Observe chat array for P2P-delivered messages and fire SSE events.
        // Local posts already fire events in post_chat(); this covers remote peers.
        let chat_arr = control.get_or_insert_array(CHAT_KEY);
        let events_for_chat = events_tx.clone();
        let seen_chat_ids = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let chat_sub = chat_arr.observe(
            move |txn: &yrs::TransactionMut, event: &yrs::types::array::ArrayEvent| {
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                let mut seen = seen_chat_ids.lock().unwrap();
                for change in event.delta(txn) {
                    if let yrs::types::Change::Added(values) = change {
                        for val in values {
                            if let yrs::Out::Any(yrs::Any::String(s)) = val {
                                if let Ok(msg) = serde_json::from_str::<ChatMessage>(s) {
                                    if !seen.insert(msg.id.clone()) || !is_p2p {
                                        continue;
                                    }
                                    let _ = events_for_chat.send(CircleEvent::MessagePosted {
                                        message: msg.clone(),
                                    });
                                    for mention in &msg.mentions {
                                        let _ = events_for_chat.send(CircleEvent::AgentMentioned {
                                            agent_id: mention.clone(),
                                            message: msg.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            },
        );
        std::mem::forget(chat_sub);

        // Chat activity is an ephemeral CRDT map. Local writers emit their own
        // event; this observer turns P2P-delivered updates into local SSE
        // notifications without adding anything to the durable transcript.
        let activity_map = control.get_or_insert_map(CHAT_ACTIVITY_KEY);
        let events_for_activity = events_tx.clone();
        let activity_sub =
            activity_map.observe(
                move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                    let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                    if !is_p2p {
                        return;
                    }
                    for change in event.keys(txn).values() {
                        let raw = match change {
                            yrs::types::EntryChange::Inserted(yrs::Out::Any(yrs::Any::String(
                                s,
                            )))
                            | yrs::types::EntryChange::Updated(
                                _,
                                yrs::Out::Any(yrs::Any::String(s)),
                            ) => Some(s),
                            _ => None,
                        };
                        if let Some(raw) = raw {
                            if let Ok(activity) = serde_json::from_str::<ChatActivity>(raw) {
                                let _ = events_for_activity
                                    .send(CircleEvent::ChatActivityChanged { activity });
                            }
                        }
                    }
                },
            );
        std::mem::forget(activity_sub);

        // Observe tasks for P2P-delivered changes and fire SSE events. Local task
        // APIs emit their own events; this covers updates that arrived via CRDT sync.
        let tasks_map = control.get_or_insert_map(TASKS_KEY);
        let events_for_tasks = events_tx.clone();
        let tasks_sub = tasks_map.observe(
            move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                if !is_p2p {
                    return;
                }

                for change in event.keys(txn).values() {
                    match change {
                        yrs::types::EntryChange::Inserted(yrs::Out::Any(yrs::Any::String(s))) => {
                            if let Ok(task) = serde_json::from_str::<Task>(s) {
                                let _ = events_for_tasks.send(CircleEvent::TaskCreated {
                                    task_id: task.task_id,
                                });
                            }
                        }
                        yrs::types::EntryChange::Updated(_, yrs::Out::Any(yrs::Any::String(s))) => {
                            if let Ok(task) = serde_json::from_str::<Task>(s) {
                                match task.status {
                                    TaskStatus::Claimed => {
                                        let _ = events_for_tasks.send(CircleEvent::TaskClaimed {
                                            task_id: task.task_id,
                                            agent_id: task.claimed_by.unwrap_or_default(),
                                        });
                                    }
                                    TaskStatus::Done => {
                                        let _ = events_for_tasks.send(CircleEvent::TaskDone {
                                            task_id: task.task_id,
                                        });
                                    }
                                    TaskStatus::Open => {
                                        let _ = events_for_tasks.send(CircleEvent::TaskCreated {
                                            task_id: task.task_id,
                                        });
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            },
        );
        std::mem::forget(tasks_sub);

        // Observe the member list for P2P-delivered changes and fire SSE events.
        // Local membership APIs emit their own events; without this, a change
        // that arrived purely by CRDT sync was applied silently, so a peer's
        // open UI kept the roster it fetched when the circle was opened. That
        // hid a device's advertised agents — the list a device republishes when
        // `agents.toml` changes — until the page was reloaded.
        //
        // `Updated` matters as much as `Inserted` here: a device that changes
        // its advertised agents rewrites an entry that already exists, so the
        // insert-only case never sees it.
        let member_map = control.get_or_insert_map(MEMBER_LIST_KEY);
        let events_for_members = events_tx.clone();
        let members_sub = member_map.observe(
            move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                if !is_p2p {
                    return;
                }
                for (key, change) in event.keys(txn).iter() {
                    let peer_id = key.to_string();
                    let event = match change {
                        yrs::types::EntryChange::Inserted(_)
                        | yrs::types::EntryChange::Updated(_, _) => {
                            CircleEvent::MemberAdded { peer_id }
                        }
                        yrs::types::EntryChange::Removed(_) => {
                            CircleEvent::MemberRemoved { peer_id }
                        }
                    };
                    let _ = events_for_members.send(event);
                }
            },
        );
        std::mem::forget(members_sub);

        // Observe presence for P2P-delivered changes, so a peer going offline or
        // coming back updates an open UI instead of waiting for a reload.
        //
        // Heartbeats rewrite an entry every 30s without changing anything a
        // viewer can see, so this fires only when `status` actually transitions.
        // Emitting per write would have every client refetch the roster on every
        // peer's heartbeat forever. A `current_file` change is likewise not a
        // status change and deliberately does not wake the roster.
        let presence_map = control.get_or_insert_map(PRESENCE_KEY);
        let events_for_presence = events_tx.clone();
        let presence_sub = presence_map.observe(
            move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                if !is_p2p {
                    return;
                }
                let parse = |out: &Out| match out {
                    Out::Any(Any::String(s)) => serde_json::from_str::<Presence>(s).ok(),
                    _ => None,
                };
                for change in event.keys(txn).values() {
                    let changed = match change {
                        yrs::types::EntryChange::Inserted(new) => parse(new),
                        yrs::types::EntryChange::Updated(old, new) => {
                            match (parse(old), parse(new)) {
                                // Same status — a heartbeat or a file-focus change.
                                (Some(before), Some(after)) if before.status == after.status => {
                                    None
                                }
                                (_, after) => after,
                            }
                        }
                        yrs::types::EntryChange::Removed(_) => None,
                    };
                    if let Some(presence) = changed {
                        let _ = events_for_presence.send(CircleEvent::PresenceChanged {
                            agent_id: presence.agent_id,
                        });
                    }
                }
            },
        );
        std::mem::forget(presence_sub);

        // Proposals are not replicated through the control doc. They sync via
        // the dedicated pull protocol (`crate::network::proposal_sync`), which
        // reconciles the disk store directly on each peer connection — keeping
        // the (in-memory, fully-replicated) control doc free of unbounded
        // proposal history.

        Self {
            circle_id,
            circle_name,
            workspace,
            circle_dir,
            admin_pubkey_hex,
            agent_id,
            session_id,
            peer_id,
            p2p_external_addrs: Arc::new(RwLock::new(Vec::new())),
            p2p_listen_addrs: Arc::new(RwLock::new(Vec::new())),
            peer_connections: Arc::new(RwLock::new(HashMap::new())),
            docs: Arc::new(DashMap::new()),
            control,
            doc_updates: Arc::new(DashMap::new()),
            awareness_updates: Arc::new(DashMap::new()),
            all_updates: all_updates_tx,
            all_awareness_updates: all_awareness_tx,
            all_deletes: all_deletes_tx,
            events: events_tx,
            interactive_writes: interactive_writes_tx,
            review_writes: review_writes_tx,
            self_write_flags: Arc::new(DashMap::new()),
            join_policy,
            owner,
            mls,
            recent_conn_errors: Arc::new(RwLock::new(std::collections::VecDeque::new())),
        }
    }

    /// Get or create a Doc for a file path, wiring up update broadcasting.
    pub fn get_or_create_doc(&self, rel_path: &str) -> Arc<Doc> {
        if let Some(doc) = self.docs.get(rel_path) {
            return doc.clone();
        }
        let doc = Arc::new(Doc::new());
        let (update_tx, _) = broadcast::channel::<Vec<u8>>(64);

        self.docs.insert(rel_path.to_string(), doc.clone());
        self.doc_updates.insert(rel_path.to_string(), update_tx);

        let tx = self.doc_updates.get(rel_path).unwrap().clone();
        let all_tx = self.all_updates.clone();
        let path_owned = rel_path.to_string();

        let sub = doc
            .observe_update_v1(move |txn, event| {
                let raw = event.update.clone();
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                // Always notify local WS subscribers.
                let _ = tx.send(raw.clone());
                // Only forward to P2P if the update was NOT from a remote peer.
                // This prevents echoing a received update back to the sender.
                if !is_p2p {
                    let _ = all_tx.send((path_owned.clone(), raw));
                }
                // CRDT state is now saved synchronously in flush_to_disk and handle_event,
                // not here, to avoid the race condition where a background save can be
                // killed mid-write if the daemon shuts down between flush_to_disk and save.
            })
            .expect("observe_update_v1 failed");

        // The Subscription token is RAII — dropping it unregisters the observer.
        // Docs live for the entire daemon lifetime, so leaking is safe here.
        std::mem::forget(sub);

        doc
    }

    pub fn subscribe_doc_updates(&self, rel_path: &str) -> broadcast::Receiver<Vec<u8>> {
        self.get_or_create_doc(rel_path); // ensure doc + channel exist
        self.doc_updates.get(rel_path).unwrap().subscribe()
    }

    /// Record a recent P2P connection failure (keeps the last 10), for the
    /// status API. Lets silent handshake failures like a PSK mismatch be seen.
    pub fn record_conn_error(&self, msg: String) {
        let mut errs = self.recent_conn_errors.write().unwrap();
        errs.push_back((chrono::Utc::now().timestamp(), msg));
        while errs.len() > 10 {
            errs.pop_front();
        }
    }

    pub fn try_is_peer_removed(&self, peer_id: &str) -> Option<bool> {
        use yrs::ReadTxn;
        let txn = self.control.try_transact().ok()?;
        let Some(removed) = txn.get_map(MLS_REMOVED_KEY) else {
            return Some(false);
        };
        Some(matches!(
            removed.get(&txn, peer_id),
            Some(Out::Any(Any::String(_)))
        ))
    }

    pub fn is_peer_removed(&self, peer_id: &str) -> bool {
        self.try_is_peer_removed(peer_id).unwrap_or(false)
    }

    pub fn is_self_removed(&self) -> bool {
        self.is_peer_removed(&self.peer_id)
    }

    pub fn record_peer_connection(
        &self,
        peer_id: String,
        connection_id: ConnectionId,
        address: &Multiaddr,
    ) {
        let connection = PeerConnection {
            kind: classify_connection_address(address),
            address: address.to_string(),
        };
        self.peer_connections
            .write()
            .unwrap()
            .entry(peer_id)
            .or_default()
            .insert(connection_id, connection);
    }

    pub fn remove_peer_connection(&self, peer_id: &str, connection_id: ConnectionId) {
        let mut peers = self.peer_connections.write().unwrap();
        if let Some(connections) = peers.get_mut(peer_id) {
            connections.remove(&connection_id);
            if connections.is_empty() {
                peers.remove(peer_id);
            }
        }
    }

    /// Return one representative active address for each route type.
    pub fn peer_connections(&self, peer_id: &str) -> Vec<PeerConnection> {
        let mut connections = self
            .peer_connections
            .read()
            .unwrap()
            .get(peer_id)
            .map(|entries| entries.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        connections.sort_by(|a, b| {
            a.kind
                .rank()
                .cmp(&b.kind.rank())
                .then_with(|| a.address.cmp(&b.address))
        });
        connections.dedup_by_key(|connection| connection.kind);
        connections
    }

    pub fn remove_doc(&self, rel_path: &str) {
        self.docs.remove(rel_path);
        self.doc_updates.remove(rel_path);
        self.awareness_updates.remove(rel_path);
        self.self_write_flags.remove(rel_path);
    }

    /// Get or create the awareness broadcast channel for a doc path.
    pub fn awareness_sender(&self, rel_path: &str) -> broadcast::Sender<Vec<u8>> {
        self.awareness_updates
            .entry(rel_path.to_string())
            .or_insert_with(|| broadcast::channel::<Vec<u8>>(64).0)
            .clone()
    }
}

fn classify_connection_address(address: &Multiaddr) -> ConnectionKind {
    if address
        .iter()
        .any(|protocol| matches!(protocol, Protocol::P2pCircuit))
    {
        return ConnectionKind::Relay;
    }

    for protocol in address.iter() {
        match protocol {
            Protocol::Ip4(ip) => {
                let octets = ip.octets();
                if octets[0] == 100 && (64..=127).contains(&octets[1]) {
                    return ConnectionKind::Tailscale;
                }
                if ip.is_private() || ip.is_loopback() || ip.is_link_local() {
                    return ConnectionKind::Lan;
                }
                return ConnectionKind::Public;
            }
            Protocol::Ip6(ip) => {
                let segments = ip.segments();
                if segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0 {
                    return ConnectionKind::Tailscale;
                }
                if ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local() {
                    return ConnectionKind::Lan;
                }
                return ConnectionKind::Public;
            }
            _ => {}
        }
    }

    // A direct DNS address is externally routable unless the circuit marker
    // above identifies it as a relayed connection.
    ConnectionKind::Public
}

#[cfg(test)]
mod tests {
    use super::{classify_connection_address, AppState, ConnectionKind};
    use crate::control::{
        AgentStatus, CircleEvent, MemberEntry, MemberRole, Presence, MEMBER_LIST_KEY, PRESENCE_KEY,
    };
    use crate::{config::JoinPolicy, mls};
    use std::path::PathBuf;
    use yrs::{Any, Map, Transact, WriteTxn};

    fn kind(address: &str) -> ConnectionKind {
        classify_connection_address(&address.parse().unwrap())
    }

    fn test_state() -> AppState {
        AppState::new(
            "circle".into(),
            "Circle".into(),
            PathBuf::new(),
            PathBuf::new(),
            String::new(),
            "agent".into(),
            1,
            "peer-local".into(),
            JoinPolicy::Manual,
            "owner".into(),
            mls::new_mls_state(mls::MlsIdentity::generate("peer-local").unwrap(), None),
        )
    }

    /// Write into a control-doc map the way CRDT sync does — with the "p2p"
    /// origin that marks an update as arriving from a peer.
    fn p2p_write(state: &AppState, map_key: &str, key: &str, json: &str) {
        let mut txn = state.control.try_transact_mut_with("p2p").unwrap();
        let map = txn.get_or_insert_map(map_key);
        map.insert(&mut txn, key, Any::String(json.into()));
    }

    fn member_json(peer_id: &str, agents: &[&str]) -> String {
        serde_json::to_string(&MemberEntry {
            peer_id: peer_id.to_string(),
            owner: "suzy".into(),
            agent_id: format!("suzy-{peer_id}"),
            device_label: "macbook-pro".into(),
            agents: agents.iter().map(|a| a.to_string()).collect(),
            role: MemberRole::Member,
            added_at: chrono::Utc::now(),
            signature: String::new(),
        })
        .unwrap()
    }

    fn presence_json(agent_id: &str, status: AgentStatus) -> String {
        serde_json::to_string(&Presence {
            agent_id: agent_id.to_string(),
            status,
            last_seen: chrono::Utc::now(),
            current_file: None,
            peer_id: "peer-remote".into(),
        })
        .unwrap()
    }

    fn drain(rx: &mut tokio::sync::broadcast::Receiver<CircleEvent>) -> Vec<CircleEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    // A device that changes its advertised agents rewrites a member entry that
    // already exists. Without an event for the update, a peer's open UI kept the
    // roster it fetched when the circle was opened, hiding the new agent.
    #[test]
    fn remote_member_update_announces_the_roster_change() {
        let state = test_state();
        let mut rx = state.events.subscribe();

        p2p_write(
            &state,
            MEMBER_LIST_KEY,
            "peer-remote",
            &member_json("peer-remote", &[]),
        );
        assert!(
            drain(&mut rx).iter().any(
                |ev| matches!(ev, CircleEvent::MemberAdded { peer_id } if peer_id == "peer-remote")
            ),
            "a new member entry should announce itself"
        );

        // The regression: same key, new value — an Updated, not an Inserted.
        p2p_write(
            &state,
            MEMBER_LIST_KEY,
            "peer-remote",
            &member_json("peer-remote", &["claude"]),
        );
        assert!(
            drain(&mut rx).iter().any(
                |ev| matches!(ev, CircleEvent::MemberAdded { peer_id } if peer_id == "peer-remote")
            ),
            "an advertised-agents change should announce itself"
        );
    }

    // A local write already fires its own event from the membership API, so
    // echoing it here would double-report every local change.
    #[test]
    fn local_member_write_is_not_announced() {
        let state = test_state();
        let mut rx = state.events.subscribe();
        {
            let mut txn = state.control.transact_mut();
            let map = txn.get_or_insert_map(MEMBER_LIST_KEY);
            let json = member_json("peer-local", &["claude"]);
            map.insert(&mut txn, "peer-local", Any::String(json.as_str().into()));
        }
        assert!(
            !drain(&mut rx)
                .iter()
                .any(|ev| matches!(ev, CircleEvent::MemberAdded { .. })),
            "local writes are announced by the API, not the observer"
        );
    }

    // Presence is rewritten every 30s by heartbeat. Announcing each one would
    // have every client refetch the roster on every peer's heartbeat forever.
    #[test]
    fn presence_announces_transitions_but_not_heartbeats() {
        let state = test_state();
        let mut rx = state.events.subscribe();

        p2p_write(
            &state,
            PRESENCE_KEY,
            "suzy-remote",
            &presence_json("suzy-remote", AgentStatus::Online),
        );
        assert_eq!(
            drain(&mut rx)
                .iter()
                .filter(|ev| matches!(ev, CircleEvent::PresenceChanged { .. }))
                .count(),
            1,
            "first sight of a peer is a transition"
        );

        // Heartbeat: same status, fresh timestamp.
        p2p_write(
            &state,
            PRESENCE_KEY,
            "suzy-remote",
            &presence_json("suzy-remote", AgentStatus::Online),
        );
        assert_eq!(
            drain(&mut rx)
                .iter()
                .filter(|ev| matches!(ev, CircleEvent::PresenceChanged { .. }))
                .count(),
            0,
            "a heartbeat that changes nothing visible must stay quiet"
        );

        p2p_write(
            &state,
            PRESENCE_KEY,
            "suzy-remote",
            &presence_json("suzy-remote", AgentStatus::Offline),
        );
        assert_eq!(
            drain(&mut rx)
                .iter()
                .filter(|ev| matches!(ev, CircleEvent::PresenceChanged { .. }))
                .count(),
            1,
            "going offline is a transition"
        );
    }

    #[test]
    fn classifies_peer_connection_routes() {
        assert_eq!(kind("/ip4/192.168.1.20/tcp/50902"), ConnectionKind::Lan);
        assert_eq!(
            kind("/ip4/100.96.12.3/tcp/50902"),
            ConnectionKind::Tailscale
        );
        assert_eq!(
            kind("/ip6/fd7a:115c:a1e0::1234/tcp/50902"),
            ConnectionKind::Tailscale,
        );
        assert_eq!(kind("/ip4/203.0.113.8/tcp/50902"), ConnectionKind::Public);
        assert_eq!(
            kind("/dns4/member.example.com/tcp/50902"),
            ConnectionKind::Public
        );
        assert_eq!(
            kind("/dns4/relay.example.com/tcp/36521/p2p/12D3KooWJ5dNQYxvLwRQFqz3YxVwvhQdJXLwRj2xByLsMvjJxSxa/p2p-circuit"),
            ConnectionKind::Relay,
        );
    }
}
