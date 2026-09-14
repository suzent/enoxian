//! Device pairing — the crypto core behind `enox link`.
//!
//! Linking a second device used to mean reading a 24-word mnemonic off one
//! screen and typing it into another. The words go through a clipboard, a shell
//! history or a chat app, and they reconstruct the *user root key* — so the cost
//! of that trip is the whole identity, not one device. Then, separately, the new
//! device still had to be invited into every circle by hand.
//!
//! This module replaces that with a short code and a confirmation number, on the
//! shape Block's Buzz uses for Nostr device pairing (NIP-AB), adapted to what
//! enoxian actually needs to move.
//!
//! # What crosses the wire
//!
//! Not the mnemonic. The new device generates its own device seed locally and
//! never sends it; what it receives is an attestation signed by the user root
//! key over the device key it made for itself, plus one ordinary invite per
//! circle. The root key never leaves the device that holds it, and the new
//! device ends up with an identity of its own that can be removed on its own.
//!
//! # The protocol
//!
//! Both sides derive everything from a short code, which is the only thing the
//! user carries between machines.
//!
//! ```text
//!   source (has the identity)              mailbox              target (new device)
//!   ────────────────────────               ───────              ───────────────────
//!   generate X25519 ephemeral
//!   generate 10-byte code secret
//!   show code  ──────────────────── the user types it ───────►  derive mailbox id
//!   poll offer slot                                             generate X25519 ephemeral
//!                                  ◄───── offer ──────────────  seal(device key, label)
//!   X25519 + HKDF ──────────────────── same SAS ──────────────► X25519 + HKDF
//!   show "731-508"                                              show "731-508"
//!
//!   [user confirms on source]
//!   seal(attestation, invites) ───► reply ─────────────────►    verify transcript
//!                                                               [user confirms on target]
//!                                                               open, then enter each circle
//! ```
//!
//! # Why the confirmation number is the security
//!
//! The mailbox id is derived from the code, so only someone holding the code can
//! find the mailbox — but that is defence in depth, not the defence. Anyone who
//! did reach the mailbox first would have to complete an X25519 exchange, and
//! their shared secret would differ, so the two screens would show different
//! numbers and the user stops. This is the same trade Bluetooth pairing, ZRTP
//! and Matrix make: it converts a network attacker into someone who needs to be
//! standing at both machines. Signal's device linking shipped without this step
//! and was later exploited with forged QR codes.
//!
//! Everything in the mailbox is sealed. The offer is sealed under a key derived
//! from the code alone, so a server operator watching the mailbox learns neither
//! the device key nor the hostname inside it; the reply is sealed under the
//! X25519 secret, which the server never has.

use anyhow::{bail, Context, Result};
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
use hkdf::Hkdf;
use rand::TryRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

/// Wire version, carried in the offer so a mismatch is a clear message rather
/// than a decryption failure.
pub const VERSION: u8 = 1;

/// Bytes of entropy in a pairing code.
///
/// Ten bytes is sixteen typed characters, which is about the limit of what
/// someone will read off one screen and into another without resenting it. The
/// code lives for [`SESSION_TIMEOUT`] and admits one offer, and the
/// confirmation number — not this — is what stops a man in the middle, so the
/// work this has to do is make the mailbox unguessable within two minutes.
const CODE_BYTES: usize = 10;

/// How long a code is good for. Matches NIP-AB's window.
pub const SESSION_TIMEOUT_SECS: u64 = 120;

/// Digits in the confirmation number. Six decimal digits is ~20 bits: an
/// attacker racing the real target has a one-in-a-million chance per attempt,
/// against a 120-second window that admits a single offer. Four digits
/// (Bluetooth's choice) is considered too few against a targeted attack;
/// decimal beats Matrix's emoji because it survives any terminal font.
const SAS_DIGITS: u32 = 6;

// ── Domain separation ─────────────────────────────────────────────────────────
//
// Every value derived from the code gets its own label, so that learning one
// cannot be turned into another.

const INFO_MAILBOX: &[u8] = b"enoxian-pair-v1/mailbox";
const INFO_OFFER_KEY: &[u8] = b"enoxian-pair-v1/offer-key";
const INFO_SESSION_ID: &[u8] = b"enoxian-pair-v1/session-id";
const INFO_SAS: &[u8] = b"enoxian-pair-v1/sas";
const INFO_REPLY_KEY: &[u8] = b"enoxian-pair-v1/reply-key";
const INFO_TRANSCRIPT: &[u8] = b"enoxian-pair-v1/transcript";

// ── The code ──────────────────────────────────────────────────────────────────

/// Crockford base32, minus the letters that read as digits. A code is typed by
/// hand off another screen, so `0`/`O` and `1`/`I`/`L` must not both exist.
const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// The secret behind a pairing session — the only thing the user carries between
/// machines. Everything else is derived from it.
#[derive(Clone)]
pub struct Code([u8; CODE_BYTES]);

impl Code {
    /// Draw a fresh code from the OS.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; CODE_BYTES];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|e| anyhow::anyhow!("reading OS entropy for a pairing code failed: {e}"))?;
        Ok(Code(bytes))
    }

    /// The code as the user sees it: sixteen characters in groups of four.
    pub fn display(&self) -> String {
        let raw = self.encode();
        raw.as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).expect("alphabet is ASCII"))
            .collect::<Vec<_>>()
            .join("-")
    }

    fn encode(&self) -> String {
        // 10 bytes = 80 bits = exactly 16 base32 characters, no padding.
        let mut out = String::with_capacity(16);
        let mut acc: u16 = 0;
        let mut bits = 0u32;
        for byte in self.0 {
            acc = (acc << 8) | u16::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                let idx = ((acc >> bits) & 0x1f) as usize;
                out.push(ALPHABET[idx] as char);
            }
        }
        out
    }

    /// Parse what the user typed. Groups, case and the spacing are all forgiven;
    /// a character that is not in the alphabet is not, because silently mapping
    /// it would turn a typo into a failure much later in the handshake.
    pub fn parse(input: &str) -> Result<Self> {
        let cleaned: String = input
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if cleaned.len() != 16 {
            bail!(
                "a pairing code is 16 characters (you gave {}) — it looks like '{}'",
                cleaned.len(),
                Code([0x1f; CODE_BYTES]).display()
            );
        }

        let mut bytes = [0u8; CODE_BYTES];
        let mut acc: u16 = 0;
        let mut bits = 0u32;
        let mut out = 0usize;
        for c in cleaned.chars() {
            let idx = ALPHABET
                .iter()
                .position(|a| *a as char == c)
                .with_context(|| format!("'{c}' is not a character a pairing code can contain"))?;
            acc = (acc << 5) | idx as u16;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                bytes[out] = ((acc >> bits) & 0xff) as u8;
                out += 1;
            }
        }
        debug_assert_eq!(out, CODE_BYTES);
        Ok(Code(bytes))
    }

    fn derive(&self, info: &[u8], out: &mut [u8]) {
        Hkdf::<Sha256>::new(None, &self.0)
            .expand(info, out)
            .expect("HKDF output length is within bounds");
    }

    /// Where the two sides meet. Derived from the code rather than carried in
    /// it, so the server learns nothing about the code from the id it is asked
    /// for — the same reason NIP-AB derives its session id instead of hashing.
    pub fn mailbox_id(&self) -> String {
        let mut out = [0u8; 32];
        self.derive(INFO_MAILBOX, &mut out);
        hex::encode(out)
    }

    /// Proves the offer came from someone holding the code, without putting the
    /// code on the wire.
    fn session_id(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        self.derive(INFO_SESSION_ID, &mut out);
        out
    }

    /// Seals the offer. Derived from the code alone, because at offer time
    /// there is no shared X25519 secret yet — its job is to keep the device key
    /// and hostname inside the offer away from whoever is storing the mailbox.
    fn offer_key(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        self.derive(INFO_OFFER_KEY, &mut out);
        out
    }
}

// ── Sealed boxes ──────────────────────────────────────────────────────────────

/// Seal `plaintext` under `key`, binding `aad` to the result.
///
/// The nonce is random and prepended. ChaCha20-Poly1305 is what the content
/// layer already uses, so this adds no new primitive to the build.
fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::SysRng
        .try_fill_bytes(&mut nonce_bytes)
        .map_err(|e| anyhow::anyhow!("reading OS entropy for a pairing nonce failed: {e}"))?;

    let cipher = ChaCha20Poly1305::new(&Key::from(*key));
    let ciphertext = cipher
        .encrypt(
            &Nonce::from(nonce_bytes),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("sealing a pairing message failed"))?;

    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Open what `seal` produced. A failure here is always reported the same way:
/// the reason is either a wrong code or a tampered message, and distinguishing
/// them for the caller would only help someone probing the mailbox.
fn open(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < 12 + 16 {
        bail!("pairing message is too short to be valid");
    }
    let (nonce_bytes, ciphertext) = sealed.split_at(12);
    let nonce: [u8; 12] = nonce_bytes
        .try_into()
        .expect("split_at(12) yields 12 bytes");
    ChaCha20Poly1305::new(&Key::from(*key))
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("pairing message did not open — check the code and try again"))
}

// ── Messages ──────────────────────────────────────────────────────────────────

/// What the new device puts in the mailbox: who it is, and the ephemeral key the
/// shared secret is built from.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub version: u8,
    /// hex — proves the sender holds the code.
    pub session_id: String,
    /// hex of the target's X25519 ephemeral public key.
    pub ephemeral_pubkey: String,
    /// hex(protobuf) of the device key this device generated for itself. The
    /// source attests it; it never sees the matching secret.
    pub device_pubkey_hex: String,
    /// What this device calls itself, e.g. "suzy-macbook".
    pub device_label: String,
}

/// What the linked device sends back once the user has confirmed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LinkPayload {
    pub version: u8,
    /// Binds the reply to this exact session. See [`Session::transcript_hash`].
    pub transcript_hash: String,
    /// The handle both devices will present, when the user has one.
    pub user_handle: Option<String>,
    /// hex of the user root public key, when the source holds a user identity.
    pub user_pubkey_hex: Option<String>,
    /// hex of the root key's signature over the target's device key and label.
    /// The root key itself is never sent.
    pub user_attestation_hex: Option<String>,
    /// One ordinary `enoxian://` invite per circle the source belongs to. The
    /// new device redeems them through the same path as a pasted invite, so
    /// admission stays the flow that already exists and is already tested.
    pub invites: Vec<String>,
}

// ── The session ───────────────────────────────────────────────────────────────

/// One side of a pairing handshake, from the ephemeral key to the shared secret.
pub struct Session {
    code: Code,
    /// Taken by `agree`, so a session cannot complete an exchange twice.
    secret: Option<StaticSecret>,
    public: PublicKey,
}

/// A session that has seen the peer's ephemeral key and can seal to it.
pub struct Agreed {
    code: Code,
    shared: [u8; 32],
    source_pubkey: [u8; 32],
    target_pubkey: [u8; 32],
}

impl Session {
    /// Start a side of the handshake with a fresh ephemeral key.
    ///
    /// The key is seeded from OS bytes rather than through x25519-dalek's own
    /// RNG traits, which track a different `rand_core` than this crate's `rand`.
    /// `StaticSecret` is the type that can be built that way; it is used here
    /// for exactly one exchange and then dropped.
    pub fn new(code: Code) -> Result<Self> {
        let mut seed = [0u8; 32];
        rand::rngs::SysRng
            .try_fill_bytes(&mut seed)
            .map_err(|e| anyhow::anyhow!("reading OS entropy for a pairing key failed: {e}"))?;
        let secret = StaticSecret::from(seed);
        let public = PublicKey::from(&secret);
        Ok(Session {
            code,
            secret: Some(secret),
            public,
        })
    }

    pub fn code(&self) -> &Code {
        &self.code
    }

    pub fn ephemeral_pubkey_hex(&self) -> String {
        hex::encode(self.public.as_bytes())
    }

    /// The raw 32 bytes, for the mailbox slot that carries this key verbatim.
    pub fn ephemeral_pubkey_bytes(&self) -> Vec<u8> {
        self.public.as_bytes().to_vec()
    }

    /// Complete the exchange against the peer's ephemeral key.
    ///
    /// `is_source` fixes the order the two public keys enter the transcript, so
    /// both sides hash the same bytes rather than each hashing "mine, then
    /// theirs".
    pub fn agree(mut self, peer_pubkey_hex: &str, is_source: bool) -> Result<Agreed> {
        let bytes =
            hex::decode(peer_pubkey_hex.trim()).context("peer ephemeral key is not valid hex")?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("peer ephemeral key is not 32 bytes"))?;
        let peer = PublicKey::from(bytes);

        let secret = self
            .secret
            .take()
            .context("this pairing session has already completed its exchange")?;
        let shared = secret.diffie_hellman(&peer);

        // An all-zero shared secret means the peer sent a small-order point,
        // which is how a key-substitution attack looks from here.
        if !shared.was_contributory() {
            bail!("peer ephemeral key is not usable for key agreement");
        }

        let (source_pubkey, target_pubkey) = if is_source {
            (*self.public.as_bytes(), bytes)
        } else {
            (bytes, *self.public.as_bytes())
        };

        Ok(Agreed {
            code: self.code,
            shared: *shared.as_bytes(),
            source_pubkey,
            target_pubkey,
        })
    }
}

impl Agreed {
    fn derive(&self, info: &[u8], out: &mut [u8]) {
        // The code is the salt, so an attacker who somehow learned the X25519
        // secret still could not derive these without having seen the code.
        Hkdf::<Sha256>::new(Some(&self.code.0), &self.shared)
            .expand(info, out)
            .expect("HKDF output length is within bounds");
    }

    fn sas_input(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        self.derive(INFO_SAS, &mut out);
        out
    }

    /// The number both screens show, zero-padded and grouped for reading aloud.
    pub fn confirmation_number(&self) -> String {
        let input = self.sas_input();
        let n =
            u32::from_be_bytes([input[0], input[1], input[2], input[3]]) % 10u32.pow(SAS_DIGITS);
        let digits = format!("{n:0width$}", width = SAS_DIGITS as usize);
        format!("{}-{}", &digits[..3], &digits[3..])
    }

    /// Commits the reply to this exact session: the code, both ephemeral keys
    /// and the confirmation number.
    ///
    /// The user comparing two screens is what stops a man in the middle; this
    /// only lets the target *detect* a session whose parameters do not match
    /// what the source thought it was confirming.
    pub fn transcript_hash(&self) -> String {
        let mut transcript = Vec::with_capacity(96);
        transcript.extend_from_slice(&self.source_pubkey);
        transcript.extend_from_slice(&self.target_pubkey);
        transcript.extend_from_slice(&self.sas_input());

        let mut out = [0u8; 32];
        Hkdf::<Sha256>::new(Some(&self.code.0), &transcript)
            .expand(INFO_TRANSCRIPT, &mut out)
            .expect("HKDF output length is within bounds");
        hex::encode(out)
    }

    fn reply_key(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        self.derive(INFO_REPLY_KEY, &mut out);
        out
    }

    /// Seal the payload for the new device.
    pub fn seal_payload(&self, payload: &LinkPayload) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(payload).context("serialize pairing payload")?;
        seal(&self.reply_key(), self.code.mailbox_id().as_bytes(), &json)
    }

    /// Open the payload, then check it is the one the source confirmed.
    ///
    /// The transcript is verified here rather than by the caller so that a
    /// payload from a mismatched session cannot be returned at all, even to a
    /// caller that forgets to look.
    pub fn open_payload(&self, sealed: &[u8]) -> Result<LinkPayload> {
        let json = open(&self.reply_key(), self.code.mailbox_id().as_bytes(), sealed)?;
        let payload: LinkPayload =
            serde_json::from_slice(&json).context("pairing payload is not valid JSON")?;

        if payload.version != VERSION {
            bail!(
                "the other device speaks pairing version {} and this one speaks {VERSION} — \
                 update whichever is older",
                payload.version
            );
        }
        if !constant_time_eq(
            payload.transcript_hash.as_bytes(),
            self.transcript_hash().as_bytes(),
        ) {
            bail!("this pairing does not match what the other device confirmed — start again");
        }
        Ok(payload)
    }
}

// ── Offer sealing ─────────────────────────────────────────────────────────────

impl Code {
    /// Seal an offer for the mailbox.
    pub fn seal_offer(&self, offer: &Offer) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(offer).context("serialize pairing offer")?;
        seal(&self.offer_key(), self.mailbox_id().as_bytes(), &json)
    }

    /// Open an offer and check it was written by someone holding this code.
    pub fn open_offer(&self, sealed: &[u8]) -> Result<Offer> {
        let json = open(&self.offer_key(), self.mailbox_id().as_bytes(), sealed)?;
        let offer: Offer = serde_json::from_slice(&json).context("offer is not valid JSON")?;

        if offer.version != VERSION {
            bail!(
                "the other device speaks pairing version {} and this one speaks {VERSION} — \
                 update whichever is older",
                offer.version
            );
        }
        if !constant_time_eq(
            offer.session_id.as_bytes(),
            hex::encode(self.session_id()).as_bytes(),
        ) {
            bail!("offer does not match this pairing code");
        }
        Ok(offer)
    }

    /// The session id an offer must carry.
    pub fn session_id_hex(&self) -> String {
        hex::encode(self.session_id())
    }
}

/// Compare without letting the time taken reveal where two values first differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn paired() -> (Agreed, Agreed) {
        let code = Code::generate().unwrap();
        let source = Session::new(code.clone()).unwrap();
        let target = Session::new(code).unwrap();
        let source_pub = source.ephemeral_pubkey_hex();
        let target_pub = target.ephemeral_pubkey_hex();
        (
            source.agree(&target_pub, true).unwrap(),
            target.agree(&source_pub, false).unwrap(),
        )
    }

    #[test]
    fn a_code_round_trips_through_what_the_user_types() {
        for _ in 0..100 {
            let code = Code::generate().unwrap();
            let shown = code.display();
            assert_eq!(shown.len(), 19, "16 characters in four groups: {shown}");
            assert_eq!(Code::parse(&shown).unwrap().0, code.0);
        }
    }

    /// People retype codes by hand. Case, spacing and the grouping dashes are
    /// all noise; none of them should be the reason a link fails.
    #[test]
    fn typing_a_code_back_is_forgiving_about_shape() {
        let code = Code::generate().unwrap();
        let shown = code.display();
        for variant in [
            shown.to_uppercase(),
            shown.replace('-', ""),
            shown.replace('-', " "),
            format!("  {shown}  "),
        ] {
            assert_eq!(
                Code::parse(&variant).unwrap().0,
                code.0,
                "failed on '{variant}'"
            );
        }
    }

    /// A character outside the alphabet is a typo, and saying so beats failing
    /// later with "the code did not work".
    #[test]
    fn a_code_with_an_impossible_character_is_refused() {
        let code = Code::generate().unwrap();
        let bad = format!("{}u", &code.display()[..18]); // 'u' is not in the alphabet
        assert!(Code::parse(&bad).is_err());
        assert!(Code::parse("too-short").is_err());
    }

    /// The alphabet must not contain a pair that looks the same on a terminal,
    /// or every ambiguous code becomes a support question.
    #[test]
    fn the_alphabet_has_no_confusable_characters() {
        let s = std::str::from_utf8(ALPHABET).unwrap();
        for c in ['o', 'i', 'l', 'u'] {
            assert!(!s.contains(c), "'{c}' reads as another character");
        }
        assert_eq!(s.len(), 32, "base32 needs exactly 32 symbols");
    }

    #[test]
    fn both_sides_reach_the_same_confirmation_number() {
        let (source, target) = paired();
        assert_eq!(source.confirmation_number(), target.confirmation_number());
        assert_eq!(source.transcript_hash(), target.transcript_hash());
    }

    /// The number is what the user reads aloud, so its shape is part of the
    /// contract: six digits, zero-padded, grouped in threes.
    #[test]
    fn the_confirmation_number_is_six_readable_digits() {
        for _ in 0..200 {
            let (source, _) = paired();
            let n = source.confirmation_number();
            assert_eq!(n.len(), 7, "expected 731-508 shape, got {n}");
            assert_eq!(&n[3..4], "-");
            assert!(n.chars().filter(char::is_ascii_digit).count() == 6);
        }
    }

    /// The whole point of the confirmation step. Someone who reaches the
    /// mailbox and substitutes their own ephemeral key gets a different shared
    /// secret, so the two screens disagree and the user stops.
    #[test]
    fn a_substituted_ephemeral_key_changes_the_number() {
        let code = Code::generate().unwrap();
        let source = Session::new(code.clone()).unwrap();
        let target = Session::new(code.clone()).unwrap();
        let attacker = Session::new(code).unwrap();

        let source_pub = source.ephemeral_pubkey_hex();
        let real = source
            .agree(&target.ephemeral_pubkey_hex(), true)
            .unwrap()
            .confirmation_number();

        // The attacker races the real target, so the source agrees with them.
        let target = target.agree(&source_pub, false).unwrap();
        let attacked = Session::new(Code::generate().unwrap()).unwrap();
        let _ = attacked;
        let attacker_view = attacker.agree(&source_pub, false).unwrap();

        assert_ne!(real, attacker_view.confirmation_number());
        assert_ne!(
            target.confirmation_number(),
            attacker_view.confirmation_number()
        );
    }

    #[test]
    fn a_payload_round_trips_between_the_two_sides() {
        let (source, target) = paired();
        let payload = LinkPayload {
            version: VERSION,
            transcript_hash: source.transcript_hash(),
            user_handle: Some("suzy".into()),
            user_pubkey_hex: Some(hex::encode([1u8; 36])),
            user_attestation_hex: Some(hex::encode([2u8; 64])),
            invites: vec!["enoxian://v2/abc".into(), "enoxian://v2/def".into()],
        };

        let sealed = source.seal_payload(&payload).unwrap();
        assert_eq!(target.open_payload(&sealed).unwrap(), payload);
    }

    /// A payload from a different session must not open, even if the attacker
    /// can put bytes in the right mailbox.
    #[test]
    fn a_payload_from_another_session_does_not_open() {
        let (source, _) = paired();
        let (_, other_target) = paired();
        let payload = LinkPayload {
            version: VERSION,
            transcript_hash: source.transcript_hash(),
            user_handle: None,
            user_pubkey_hex: None,
            user_attestation_hex: None,
            invites: vec![],
        };
        let sealed = source.seal_payload(&payload).unwrap();
        assert!(other_target.open_payload(&sealed).is_err());
    }

    /// The transcript check is what catches a payload whose session parameters
    /// are not the ones the source confirmed.
    #[test]
    fn a_payload_with_the_wrong_transcript_is_refused() {
        let (source, target) = paired();
        let payload = LinkPayload {
            version: VERSION,
            transcript_hash: "00".repeat(32),
            user_handle: None,
            user_pubkey_hex: None,
            user_attestation_hex: None,
            invites: vec![],
        };
        let sealed = source.seal_payload(&payload).unwrap();
        let err = target.open_payload(&sealed).unwrap_err();
        assert!(err.to_string().contains("confirmed"), "got: {err}");
    }

    #[test]
    fn an_offer_round_trips_and_is_bound_to_its_code() {
        let code = Code::generate().unwrap();
        let offer = Offer {
            version: VERSION,
            session_id: code.session_id_hex(),
            ephemeral_pubkey: hex::encode([3u8; 32]),
            device_pubkey_hex: hex::encode([4u8; 36]),
            device_label: "suzy-macbook".into(),
        };

        let sealed = code.seal_offer(&offer).unwrap();
        assert_eq!(code.open_offer(&sealed).unwrap(), offer);

        // A different code neither finds the mailbox nor opens the offer.
        let other = Code::generate().unwrap();
        assert_ne!(other.mailbox_id(), code.mailbox_id());
        assert!(other.open_offer(&sealed).is_err());
    }

    /// The offer carries a device key and a hostname. Whoever stores the
    /// mailbox must not be able to read either.
    #[test]
    fn an_offer_reveals_nothing_to_whoever_holds_the_mailbox() {
        let code = Code::generate().unwrap();
        let offer = Offer {
            version: VERSION,
            session_id: code.session_id_hex(),
            ephemeral_pubkey: hex::encode([3u8; 32]),
            device_pubkey_hex: hex::encode([4u8; 36]),
            device_label: "suzy-macbook".into(),
        };
        let sealed = code.seal_offer(&offer).unwrap();
        let haystack = String::from_utf8_lossy(&sealed);
        assert!(!haystack.contains("suzy-macbook"));
        assert!(!haystack.contains(&hex::encode([4u8; 36])));
    }

    /// The mailbox id goes to the server in the clear, so it must not be the
    /// code — nor let the code be worked back out of it.
    #[test]
    fn the_mailbox_id_is_not_the_code() {
        let code = Code::generate().unwrap();
        let id = code.mailbox_id();
        // 32 bytes of HKDF, hex-encoded — the shape `pair_mailbox` validates.
        assert_eq!(id.len(), 64);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(!id.contains(&code.encode()));
        assert_ne!(id, hex::encode(code.0));
        assert_ne!(id, code.session_id_hex());
    }

    /// An offer whose session id is wrong was not written by a code holder.
    #[test]
    fn an_offer_with_the_wrong_session_id_is_refused() {
        let code = Code::generate().unwrap();
        let offer = Offer {
            version: VERSION,
            session_id: "00".repeat(32),
            ephemeral_pubkey: hex::encode([3u8; 32]),
            device_pubkey_hex: hex::encode([4u8; 36]),
            device_label: "x".into(),
        };
        let sealed = code.seal_offer(&offer).unwrap();
        assert!(code.open_offer(&sealed).is_err());
    }

    #[test]
    fn a_version_mismatch_says_so_plainly() {
        let code = Code::generate().unwrap();
        let offer = Offer {
            version: VERSION + 1,
            session_id: code.session_id_hex(),
            ephemeral_pubkey: hex::encode([3u8; 32]),
            device_pubkey_hex: hex::encode([4u8; 36]),
            device_label: "x".into(),
        };
        let sealed = code.seal_offer(&offer).unwrap();
        let err = code.open_offer(&sealed).unwrap_err();
        assert!(err.to_string().contains("update"), "got: {err}");
    }

    /// Tampering with a sealed message must fail the tag, not produce garbage.
    #[test]
    fn a_tampered_message_does_not_open() {
        let (source, target) = paired();
        let payload = LinkPayload {
            version: VERSION,
            transcript_hash: source.transcript_hash(),
            user_handle: None,
            user_pubkey_hex: None,
            user_attestation_hex: None,
            invites: vec!["enoxian://v2/abc".into()],
        };
        let sealed = source.seal_payload(&payload).unwrap();
        for i in [0usize, 12, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[i] ^= 0x01;
            assert!(
                target.open_payload(&bad).is_err(),
                "byte {i} went unnoticed"
            );
        }
        assert!(target.open_payload(&sealed[..8]).is_err());
    }

    /// A small-order peer key forces a predictable shared secret. Rejecting it
    /// is what keeps the confirmation number meaningful.
    #[test]
    fn a_small_order_peer_key_is_rejected() {
        let session = Session::new(Code::generate().unwrap()).unwrap();
        assert!(session.agree(&hex::encode([0u8; 32]), true).is_err());
    }

    #[test]
    fn a_malformed_peer_key_is_rejected() {
        let code = Code::generate().unwrap();
        assert!(Session::new(code.clone())
            .unwrap()
            .agree("not hex", true)
            .is_err());
        assert!(Session::new(code)
            .unwrap()
            .agree(&hex::encode([1u8; 16]), true)
            .is_err());
    }
}
