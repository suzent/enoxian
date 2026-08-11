use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::sync::atomic::AtomicBool;
use dashmap::DashMap;
use libp2p::{multiaddr::Protocol, swarm::ConnectionId, Multiaddr};
use serde::Serialize;
use tokio::sync::broadcast;
use yrs::{Doc, Observable};
use crate::control::{CHAT_KEY, ChatMessage, CircleEvent, TASKS_KEY, Task, TaskStatus};

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
    pub fn new(circle_id: String, circle_name: String, workspace: PathBuf, circle_dir: PathBuf, admin_pubkey_hex: String, agent_id: String, session_id: u64, peer_id: String, join_policy: crate::config::JoinPolicy, owner: String, mls: crate::mls::SharedMlsState) -> Self {
        let (events_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (interactive_writes_tx, _): (broadcast::Sender<(String, Option<String>)>, _) = broadcast::channel(EVENT_CAPACITY);
        let (review_writes_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (all_updates_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (all_awareness_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (all_deletes_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let control = Arc::new(Doc::new());

        // Forward control doc updates to P2P peers (skip updates that arrived from peers).
        let all_tx = all_updates_tx.clone();
        let sub = control.observe_update_v1(move |txn, event| {
            let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
            if !is_p2p {
                let _ = all_tx.send(("__control__".to_string(), event.update.clone()));
            }
        }).expect("observe control doc failed");
        std::mem::forget(sub);

        // Observe chat array for P2P-delivered messages and fire SSE events.
        // Local posts already fire events in post_chat(); this covers remote peers.
        let chat_arr = control.get_or_insert_array(CHAT_KEY);
        let events_for_chat = events_tx.clone();
        let chat_sub = chat_arr.observe(move |txn: &yrs::TransactionMut, event: &yrs::types::array::ArrayEvent| {
            let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
            if !is_p2p { return; }
            for change in event.delta(txn) {
                if let yrs::types::Change::Added(values) = change {
                    for val in values {
                        if let yrs::Out::Any(yrs::Any::String(s)) = val {
                            if let Ok(msg) = serde_json::from_str::<ChatMessage>(s) {
                                let _ = events_for_chat.send(CircleEvent::MessagePosted { message: msg.clone() });
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
        });
        std::mem::forget(chat_sub);

        // Observe tasks for P2P-delivered changes and fire SSE events. Local task
        // APIs emit their own events; this covers updates that arrived via CRDT sync.
        let tasks_map = control.get_or_insert_map(TASKS_KEY);
        let events_for_tasks = events_tx.clone();
        let tasks_sub = tasks_map.observe(move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
            let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
            if !is_p2p { return; }

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
        });
        std::mem::forget(tasks_sub);

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

        let sub = doc.observe_update_v1(move |txn, event| {
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
        }).expect("observe_update_v1 failed");

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
                if segments[0] == 0xfd7a
                    && segments[1] == 0x115c
                    && segments[2] == 0xa1e0
                {
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
    use super::{classify_connection_address, ConnectionKind};

    fn kind(address: &str) -> ConnectionKind {
        classify_connection_address(&address.parse().unwrap())
    }

    #[test]
    fn classifies_peer_connection_routes() {
        assert_eq!(kind("/ip4/192.168.1.20/tcp/50902"), ConnectionKind::Lan);
        assert_eq!(kind("/ip4/100.96.12.3/tcp/50902"), ConnectionKind::Tailscale);
        assert_eq!(
            kind("/ip6/fd7a:115c:a1e0::1234/tcp/50902"),
            ConnectionKind::Tailscale,
        );
        assert_eq!(kind("/ip4/203.0.113.8/tcp/50902"), ConnectionKind::Public);
        assert_eq!(kind("/dns4/member.example.com/tcp/50902"), ConnectionKind::Public);
        assert_eq!(
            kind("/dns4/relay.example.com/tcp/36521/p2p/12D3KooWJ5dNQYxvLwRQFqz3YxVwvhQdJXLwRj2xByLsMvjJxSxa/p2p-circuit"),
            ConnectionKind::Relay,
        );
    }
}
