use axum::{
    extract::{Query, State, WebSocketUpgrade},
    extract::ws::{Message as WsMsg, WebSocket},
    response::Response,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use yrs::sync::protocol::{Message, SyncMessage};
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
use yrs::updates::decoder::Decode;
use yrs::{ReadTxn, Transact, Update};
use crate::state::AppState;
use crate::store::fs::flush_to_disk;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Deserialize)]
pub struct WsParams {
    pub path: String,
}

pub async fn ws_yjs_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<WsParams>,
) -> Response {
    let path = params.path.clone();
    ws.on_upgrade(move |socket| handle_socket(socket, state, path))
}

async fn handle_socket(socket: WebSocket, state: AppState, doc_path: String) {
    let doc = state.get_or_create_doc(&doc_path);
    let self_write_flag = Arc::new(AtomicBool::new(false));
    let (mut sender, mut receiver) = socket.split();

    // ── Handshake: send SyncStep1 (our state vector) ────────────────────────
    {
        let sv = doc.transact().state_vector();
        let msg = Message::Sync(SyncMessage::SyncStep1(sv));
        let mut enc = EncoderV1::new();
        msg.encode(&mut enc);
        if sender.send(WsMsg::Binary(enc.to_vec().into())).await.is_err() {
            return;
        }
    }

    // ── Subscribe to doc updates (from watcher or other WS clients) ─────────
    let mut update_rx = state.subscribe_doc_updates(&doc_path);

    // ── Main loop ────────────────────────────────────────────────────────────
    loop {
        tokio::select! {
            // Outbound: a local update → forward to this WS client as Update msg
            Ok(raw_update) = update_rx.recv() => {
                let msg = Message::Sync(SyncMessage::Update(raw_update));
                let mut enc = EncoderV1::new();
                msg.encode(&mut enc);
                if sender.send(WsMsg::Binary(enc.to_vec().into())).await.is_err() {
                    break;
                }
            }

            // Inbound: message from this WS client
            maybe_msg = receiver.next() => {
                match maybe_msg {
                    Some(Ok(WsMsg::Binary(data))) => {
                        handle_incoming(&doc, &data, &mut sender, &state, &doc_path, &self_write_flag).await;
                    }
                    Some(Ok(WsMsg::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

async fn handle_incoming(
    doc: &yrs::Doc,
    data: &[u8],
    sender: &mut futures::stream::SplitSink<WebSocket, WsMsg>,
    state: &AppState,
    doc_path: &str,
    self_write_flag: &Arc<AtomicBool>,
) {
    // Decode the y-sync message
    let mut decoder = yrs::updates::decoder::DecoderV1::new(
        yrs::encoding::read::Cursor::new(data)
    );
    let msg = match Message::decode(&mut decoder) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("ws_yjs decode error for {doc_path}: {e}");
            return;
        }
    };

    match msg {
        // Peer sends us their state vector → reply with everything they're missing
        Message::Sync(SyncMessage::SyncStep1(sv)) => {
            let diff = doc.transact().encode_diff_v1(&sv);
            let reply = Message::Sync(SyncMessage::SyncStep2(diff));
            let mut enc = EncoderV1::new();
            reply.encode(&mut enc);
            let _ = sender.send(WsMsg::Binary(enc.to_vec().into())).await;
        }

        // Peer sends us a diff or incremental update → apply it
        Message::Sync(SyncMessage::SyncStep2(raw))
        | Message::Sync(SyncMessage::Update(raw)) => {
            match Update::decode_v1(&raw) {
                Ok(update) => {
                    let mut txn = doc.transact_mut();
                    if let Err(e) = txn.apply_update(update) {
                        tracing::warn!("apply_update error for {doc_path}: {e}");
                    }
                    // txn drop triggers observe_update_v1 → broadcasts to other subscribers
                }
                Err(e) => tracing::warn!("decode update error for {doc_path}: {e}"),
            }
            // Flush updated doc text to disk
            self_write_flag.store(false, Ordering::SeqCst); // reset before flush
            flush_to_disk(state, doc_path, self_write_flag).await;
        }

        // Awareness / custom messages — ignore for now
        _ => {}
    }
}
