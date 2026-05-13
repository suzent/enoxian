use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use dashmap::DashMap;
use tokio::sync::broadcast;
use yrs::Doc;
use crate::control::CircleEvent;

pub const EVENT_CAPACITY: usize = 256;

/// Shared state — Clone is cheap (all fields are Arc).
#[derive(Clone)]
pub struct AppState {
    pub circle_id: String,
    pub circle_name: String,
    pub workspace: PathBuf,
    /// File docs. Key = relative path with forward slashes.
    pub docs: Arc<DashMap<String, Arc<Doc>>>,
    /// __control__ coordination document
    pub control: Arc<Doc>,
    /// Per-doc raw v1 update bytes broadcast (for WS clients and local subscribers)
    pub doc_updates: Arc<DashMap<String, broadcast::Sender<Vec<u8>>>>,
    /// Global broadcast: (rel_path, raw_v1_update). Used by P2P sync to forward local updates.
    pub all_updates: broadcast::Sender<(String, Vec<u8>)>,
    /// SSE event stream
    pub events: broadcast::Sender<CircleEvent>,
    /// Per-path flag: set to true before flush_to_disk writes, cleared by watcher on receipt.
    /// Shared between the file watcher and flush_to_disk so they operate on the same flag.
    pub self_write_flags: Arc<DashMap<String, Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(circle_id: String, circle_name: String, workspace: PathBuf) -> Self {
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

        Self {
            circle_id,
            circle_name,
            workspace,
            docs: Arc::new(DashMap::new()),
            control,
            doc_updates: Arc::new(DashMap::new()),
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
}
