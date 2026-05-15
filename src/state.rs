use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use dashmap::DashMap;
use tokio::sync::broadcast;
use yrs::{Doc, Observable};
use crate::control::{CHAT_KEY, ChatMessage, CircleEvent};

pub const EVENT_CAPACITY: usize = 256;

/// Shared state — Clone is cheap (all fields are Arc).
#[derive(Clone)]
pub struct AppState {
    pub circle_id: String,
    pub circle_name: String,
    pub workspace: PathBuf,
    pub admin_pubkey_hex: String,
    pub agent_id: String,
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
    /// SSE event stream
    pub events: broadcast::Sender<CircleEvent>,
    /// Per-path flag: set to true before flush_to_disk writes, cleared by watcher on receipt.
    /// Shared between the file watcher and flush_to_disk so they operate on the same flag.
    pub self_write_flags: Arc<DashMap<String, Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(circle_id: String, circle_name: String, workspace: PathBuf, admin_pubkey_hex: String, agent_id: String) -> Self {
        let (events_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (all_updates_tx, _) = broadcast::channel(EVENT_CAPACITY);
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
                            if let Ok(msg) = serde_json::from_str::<ChatMessage>(&s) {
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

        Self {
            circle_id,
            circle_name,
            workspace,
            admin_pubkey_hex,
            agent_id,
            docs: Arc::new(DashMap::new()),
            control,
            doc_updates: Arc::new(DashMap::new()),
            awareness_updates: Arc::new(DashMap::new()),
            all_updates: all_updates_tx,
            events: events_tx,
            self_write_flags: Arc::new(DashMap::new()),
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
        let doc_weak = Arc::downgrade(&doc);
        let workspace = self.workspace.clone();
        let crdt_rel = rel_path.to_string();

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
            // Persist CRDT state so restarts don't generate new operation IDs.
            if let Some(doc) = doc_weak.upgrade() {
                let ws = workspace.clone();
                let rp = crdt_rel.clone();
                tokio::spawn(async move {
                    crate::store::crdt::save(&ws, &rp, &doc).await;
                });
            }
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

    /// Get or create the awareness broadcast channel for a doc path.
    pub fn awareness_sender(&self, rel_path: &str) -> broadcast::Sender<Vec<u8>> {
        if let Some(tx) = self.awareness_updates.get(rel_path) {
            return tx.clone();
        }
        let (tx, _) = broadcast::channel::<Vec<u8>>(64);
        self.awareness_updates.insert(rel_path.to_string(), tx.clone());
        tx
    }
}
