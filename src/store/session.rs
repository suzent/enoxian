use std::path::Path;

/// Load the current session counter for a circle, increment it, persist it,
/// and return the new value. Each daemon start gets a strictly increasing ID.
pub async fn next_session_id(circle_dir: &Path) -> u64 {
    let path = circle_dir.join("session_id");
    let prev: u64 = tokio::fs::read_to_string(&path)
        .await
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let next = prev + 1;
    let _ = tokio::fs::write(&path, next.to_string()).await;
    next
}

/// Record the session ID last seen from a specific peer, plus a timestamp.
/// Stored as `peers/<peer_id>` inside the circle dir.
pub async fn record_peer(circle_dir: &Path, peer_id: &str, session_id: u64, connected_at: i64) {
    let dir = circle_dir.join("peers");
    let _ = tokio::fs::create_dir_all(&dir).await;
    let line = format!("{session_id}\n{connected_at}\n");
    let _ = tokio::fs::write(dir.join(peer_id), line).await;
}

/// Load the last-seen session ID and connected_at timestamp for a peer.
/// Returns None if we have never connected to this peer.
pub async fn load_peer(circle_dir: &Path, peer_id: &str) -> Option<(u64, i64)> {
    let path = circle_dir.join("peers").join(peer_id);
    let content = tokio::fs::read_to_string(&path).await.ok()?;
    let mut lines = content.lines();
    let session_id: u64 = lines.next()?.trim().parse().ok()?;
    let connected_at: i64 = lines.next()?.trim().parse().ok()?;
    Some((session_id, connected_at))
}
