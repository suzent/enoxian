//! M17 authenticated content frames derived from the current MLS epoch.

use anyhow::Result;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use std::time::Duration;

use crate::state::AppState;

const MAGIC: &[u8; 8] = b"ENOXC17\0";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 8 + 1 + 1 + 8 + 12;
const TAG_LEN: usize = 16;
const KEY_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Crdt = 1,
    Proposal = 2,
    WorkspaceEvent = 3,
}

impl TryFrom<u8> for FrameKind {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Crdt),
            2 => Ok(Self::Proposal),
            3 => Ok(Self::WorkspaceEvent),
            _ => anyhow::bail!("unknown encrypted content frame kind {value}"),
        }
    }
}

fn derive_key(root: &[u8; 32], circle_id: &str, kind: FrameKind) -> Result<[u8; 32]> {
    let hkdf = Hkdf::<Sha256>::new(Some(circle_id.as_bytes()), root);
    let mut key = [0; 32];
    let info = [b"enoxian-content-frame-v1/".as_slice(), &[kind as u8]].concat();
    hkdf.expand(&info, &mut key)
        .map_err(|_| anyhow::anyhow!("content key derivation failed"))?;
    Ok(key)
}

fn aad(header: &[u8], circle_id: &str) -> Vec<u8> {
    [header, circle_id.as_bytes()].concat()
}

async fn current_secret(state: &AppState) -> Result<(u64, [u8; 32])> {
    let deadline = tokio::time::Instant::now() + KEY_WAIT;
    loop {
        {
            let mut mls = state.mls.lock().await;
            if let Some(secret) = mls.refresh_content_secret()? {
                return Ok(secret);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "timed out waiting for MLS membership bootstrap"
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn secret_for_epoch(state: &AppState, epoch: u64) -> Result<[u8; 32]> {
    let deadline = tokio::time::Instant::now() + KEY_WAIT;
    loop {
        {
            let mut mls = state.mls.lock().await;
            if let Ok(secret) = mls.content_secret_for_epoch(epoch) {
                return Ok(secret);
            }
            if mls.current_epoch().is_some_and(|current| current > epoch) {
                anyhow::bail!("expired MLS content key for epoch {epoch}");
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for MLS epoch {epoch}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub async fn seal(state: &AppState, kind: FrameKind, plaintext: &[u8]) -> Result<Vec<u8>> {
    let (epoch, root) = current_secret(state).await?;
    seal_with_secret(&state.circle_id, kind, epoch, &root, plaintext)
}

pub async fn open(state: &AppState, expected: FrameKind, frame: &[u8]) -> Result<Vec<u8>> {
    let parsed = parse_header(frame)?;
    anyhow::ensure!(
        parsed.kind == expected,
        "encrypted content frame kind mismatch"
    );
    let root = secret_for_epoch(state, parsed.epoch).await?;
    open_with_secret(&state.circle_id, &root, frame)
}

struct ParsedHeader<'a> {
    kind: FrameKind,
    epoch: u64,
    nonce: &'a [u8],
    header: &'a [u8],
    ciphertext: &'a [u8],
}

fn parse_header(frame: &[u8]) -> Result<ParsedHeader<'_>> {
    anyhow::ensure!(
        frame.len() >= HEADER_LEN + TAG_LEN,
        "encrypted frame truncated"
    );
    anyhow::ensure!(&frame[..8] == MAGIC, "invalid encrypted frame magic");
    anyhow::ensure!(frame[8] == VERSION, "unsupported encrypted frame version");
    let kind = FrameKind::try_from(frame[9])?;
    let epoch = u64::from_be_bytes(frame[10..18].try_into().unwrap());
    Ok(ParsedHeader {
        kind,
        epoch,
        nonce: &frame[18..30],
        header: &frame[..HEADER_LEN],
        ciphertext: &frame[HEADER_LEN..],
    })
}

fn seal_with_secret(
    circle_id: &str,
    kind: FrameKind,
    epoch: u64,
    root: &[u8; 32],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let key = derive_key(root, circle_id, kind)?;
    let cipher = ChaCha20Poly1305::new((&key).into());
    let mut nonce = [0; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut frame = Vec::with_capacity(HEADER_LEN + plaintext.len() + TAG_LEN);
    frame.extend_from_slice(MAGIC);
    frame.push(VERSION);
    frame.push(kind as u8);
    frame.extend_from_slice(&epoch.to_be_bytes());
    frame.extend_from_slice(&nonce);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad(&frame, circle_id),
            },
        )
        .map_err(|_| anyhow::anyhow!("content encryption failed"))?;
    frame.extend_from_slice(&ciphertext);
    Ok(frame)
}

fn open_with_secret(circle_id: &str, root: &[u8; 32], frame: &[u8]) -> Result<Vec<u8>> {
    let parsed = parse_header(frame)?;
    let key = derive_key(root, circle_id, parsed.kind)?;
    let cipher = ChaCha20Poly1305::new((&key).into());
    cipher
        .decrypt(
            Nonce::from_slice(parsed.nonce),
            Payload {
                msg: parsed.ciphertext,
                aad: &aad(parsed.header, circle_id),
            },
        )
        .map_err(|_| anyhow::anyhow!("content authentication failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tamper_detection() {
        let root = [7; 32];
        let frame =
            seal_with_secret("circle", FrameKind::WorkspaceEvent, 42, &root, b"secret").unwrap();
        assert_eq!(
            open_with_secret("circle", &root, &frame).unwrap(),
            b"secret"
        );
        let mut tampered = frame;
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open_with_secret("circle", &root, &tampered).is_err());
    }

    #[test]
    fn purpose_circle_and_epoch_are_authenticated_or_domain_separated() {
        let root = [9; 32];
        let frame = seal_with_secret("a", FrameKind::Proposal, 5, &root, b"payload").unwrap();
        assert!(open_with_secret("b", &root, &frame).is_err());
        let parsed = parse_header(&frame).unwrap();
        assert_eq!(parsed.kind, FrameKind::Proposal);
        assert_eq!(parsed.epoch, 5);

        let mut wrong_kind = frame;
        wrong_kind[9] = FrameKind::Crdt as u8;
        assert!(open_with_secret("a", &root, &wrong_kind).is_err());
    }
}
