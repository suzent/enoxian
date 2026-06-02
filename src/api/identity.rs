use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;

use crate::identity::{DeviceIdentity, UserIdentity};

pub async fn get_identity() -> impl IntoResponse {
    match DeviceIdentity::load() {
        Ok(d) => Json(json!({
            "device_label": d.device_label,
            "user_handle":  d.user_handle,
            "has_user_key": d.user_pubkey_hex.is_some(),
        })).into_response(),
        Err(_) => Json(json!({
            "device_label": "",
            "user_handle":  null,
            "has_user_key": false,
        })).into_response(),
    }
}

#[derive(Deserialize)]
pub struct SetIdentityRequest {
    pub device_label: Option<String>,
    pub user_handle:  Option<String>,
}

pub async fn set_identity(Json(req): Json<SetIdentityRequest>) -> impl IntoResponse {
    let mut device = match DeviceIdentity::load() {
        Ok(d) => d,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };
    if let Some(label) = req.device_label {
        let label = label.trim().to_string();
        if !label.is_empty() { device.device_label = label; }
    }
    if let Some(handle) = req.user_handle {
        let handle = handle.trim().to_string();
        if handle.is_empty() {
            device.user_handle = None;
        } else {
            device.set_user_handle(handle);
        }
    }
    if let Err(e) = device.save() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }
    Json(json!({"status": "ok", "note": "restart enoxd for agent_id to reflect changes"})).into_response()
}

#[derive(Deserialize)]
pub struct LinkDeviceRequest {
    pub handle:   String,
    pub mnemonic: String,
}

/// Link this device to an existing user identity via BIP-39 mnemonic.
pub async fn link_device(Json(req): Json<LinkDeviceRequest>) -> impl IntoResponse {
    let mut device = match DeviceIdentity::load() {
        Ok(d) => d,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };
    let user = match UserIdentity::from_mnemonic(&req.mnemonic, req.handle.clone()) {
        Ok(u) => u,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("invalid mnemonic: {e}")}))).into_response(),
    };
    if let Err(e) = user.link_device(&mut device, &req.mnemonic) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }
    Json(json!({"status": "linked", "user_handle": req.handle})).into_response()
}

/// Create a brand-new user identity bound to this device.
/// Returns the BIP-39 mnemonic the user MUST back up to link other devices later.
pub async fn create_user_identity(Json(req): Json<SetIdentityRequest>) -> impl IntoResponse {
    let handle = match req.user_handle {
        Some(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": "user_handle required"}))).into_response(),
    };
    let mut device = match DeviceIdentity::load() {
        Ok(d) => d,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };
    let (user, mnemonic) = match UserIdentity::generate(handle.clone()) {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };
    if let Err(e) = user.link_device(&mut device, &mnemonic) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }
    Json(json!({
        "status":   "created",
        "handle":   handle,
        "mnemonic": mnemonic,
        "note":     "back up your mnemonic — you need it to link other devices"
    })).into_response()
}
