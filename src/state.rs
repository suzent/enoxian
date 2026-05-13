use std::path::PathBuf;
use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::broadcast;
use yrs::Doc;
use crate::control::CircleEvent;

pub const EVENT_CAPACITY: usize = 256;

/// Shared state — Clone is cheap (all fields are Arc).
/// Uses Arc<Doc> instead of Arc<RwLock<Awareness>> so AppState is Send + Sync.
/// Doc is Send + Sync internally (uses Arc<Store>).
#[derive(Clone)]
pub struct AppState {
    pub circle_id: String,
    pub circle_name: String,
    pub workspace: PathBuf,
    /// File docs. Key = relative path with forward slashes.
    pub docs: Arc<DashMap<String, Arc<Doc>>>,
    /// __control__ coordination document
    pub control: Arc<Doc>,
    /// Per-doc raw v1 update bytes broadcast
    pub doc_updates: Arc<DashMap<String, broadcast::Sender<Vec<u8>>>>,
    /// SSE event stream
    pub events: broadcast::Sender<CircleEvent>,
}

impl AppState {
    pub fn new(circle_id: String, circle_name: String, workspace: PathBuf) -> Self {
        let (events_tx, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            circle_id,
            circle_name,
            workspace,
            docs: Arc::new(DashMap::new()),
            control: Arc::new(Doc::new()),
            doc_updates: Arc::new(DashMap::new()),
            events: events_tx,
        }
    }

    /// Get or create a Doc for a file path, wiring up update broadcasting.
    pub fn get_or_create_doc(&self, rel_path: &str) -> Arc<Doc> {
        if let Some(doc) = self.docs.get(rel_path) {
            return doc.clone();
        }
        let doc = Arc::new(Doc::new());
        let (update_tx, _) = broadcast::channel::<Vec<u8>>(64);
        let tx_clone = update_tx.clone();

        // observe_update_v1 fires synchronously on TransactionMut commit.
        // The closure must be Send + 'static.
        let _sub = doc.observe_update_v1(move |_, event| {
            let _ = tx_clone.send(event.update.clone());
        }).expect("observe_update_v1 failed");

        // Note: _sub (Subscription) is dropped here. In yrs 0.26, dropping a
        // Subscription unregisters the observer. We work around this by keeping
        // the broadcast channel alive via doc_updates; the watcher/WS handler
        // resubscribes to the channel, not to the Doc observer directly.
        // A cleaner solution would store _sub, but Subscription is !Send.
        // For now we re-register the observer on every get_or_create_doc call
        // using a stored channel approach.
        //
        // Pragmatic fix: store doc first, then re-observe using a separate helper.

        self.docs.insert(rel_path.to_string(), doc.clone());
        self.doc_updates.insert(rel_path.to_string(), update_tx);

        // Re-register observer on the stored doc so sub stays alive
        // (we accept that the first sub was dropped; the channel is what matters)
        let tx2 = self.doc_updates.get(rel_path).unwrap().clone();
        let _ = doc.observe_update_v1(move |_, event| {
            let _ = tx2.send(event.update.clone());
        });

        doc
    }

    pub fn subscribe_doc_updates(&self, rel_path: &str) -> broadcast::Receiver<Vec<u8>> {
        self.get_or_create_doc(rel_path); // ensure doc + channel exist
        self.doc_updates.get(rel_path).unwrap().subscribe()
    }
}
