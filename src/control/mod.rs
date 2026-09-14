pub mod arbitration;
pub mod fs_lock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const LOCK_LOG_KEY: &str = "lock_log";
pub const TASKS_KEY: &str = "tasks";
pub const PRESENCE_KEY: &str = "presence";
/// Ephemeral chat activity (typing and agent run state). Replicated through
/// the control CRDT, but deliberately omitted from the on-disk snapshot.
pub const CHAT_ACTIVITY_KEY: &str = "chat_activity";
pub const MEMBER_LIST_KEY: &str = "member_list";
pub const CHAT_KEY: &str = "chat";
/// Delegation cascades a person has halted: `root message id -> stopped-at ts`.
///
/// This lives in the synced control doc rather than in one daemon's memory
/// because a cascade hops devices — the machine that would run the *next* turn
/// is usually not the machine whose user hit stop. Entries are dropped once no
/// live cascade could still reference them.
pub const RELAY_STOPS_KEY: &str = "relay_stops";
/// Follow-up windows a speaker has dismissed: `peer id -> dismissed-at ts`.
///
/// Like [`RELAY_STOPS_KEY`] this is synced rather than local, because the
/// device that would route a follow-up is not necessarily the device whose user
/// pressed Esc — the agent may be running on another machine entirely.
pub const ENGAGEMENT_EXITS_KEY: &str = "engagement_exits";
/// Path deletions, as a CRDT map of `rel_path -> Deletion`.
///
/// Deletion used to exist only as a live broadcast frame, which meant it had no
/// durable representation: a peer that was disconnected when a file was removed
/// never learned, and worse, still advertised the file on the next handshake
/// and re-created it on the peer that deleted it. Recording deletions in the
/// control doc makes them replicate and reconcile like any other state.
pub const DELETIONS_KEY: &str = "deletions";

// ── MLS delivery-service keys (M11) ───────────────────────────────────────
// Stored in the __control__ Yjs map; replicated to all peers via CRDT sync.

/// Map[peer_id → hex(KeyPackage TLS bytes)] — each peer publishes on daemon start.
pub const MLS_KEY_PACKAGES_KEY: &str = "mls_key_packages";
/// Map[peer_id → hex(Welcome TLS bytes)] — admin stores after `member add`.
pub const MLS_WELCOMES_KEY: &str = "mls_welcomes";
/// Array[MlsCommitEntry] — every Commit stored so offline members can catch up.
pub const MLS_COMMITS_KEY: &str = "mls_commits";
/// Map[peer_id → PendingEntry JSON] — peers waiting for admin approval.
pub const MLS_PENDING_KEY: &str = "mls_pending";
/// Map[peer_id → OwnerClaim JSON] — self-signed owner name claims.
pub const MLS_OWNER_CLAIMS_KEY: &str = "mls_owner_claims";
/// Map[peer_id → RFC-3339 timestamp] — peers that have been explicitly removed.
/// Used as a sync-level gate: removed peers are rejected before any CRDT data
/// is exchanged, even during the brief window before PSK rotation completes.
pub const MLS_REMOVED_KEY: &str = "mls_removed";
/// Map[user_pubkey_hex → DistrustEntry JSON] — user identities this circle has
/// disowned.
///
/// Scoped to the circle on purpose. The obvious place to revoke an identity is
/// the user root key that issued it, but in the case that motivates revocation
/// — a lost or stolen device — the root key is *on that device*, so whoever has
/// it can revoke the rightful owner just as easily. An admin of a circle can
/// always speak for that circle, and that is where the damage lands.
///
/// Distrusting an identity disowns every device proving it, present and future,
/// because a device is only ever as trusted as the identity behind it. That is
/// the point: an attacker holding the root key can mint new devices, and
/// removing them one peer at a time never finishes.
pub const DISTRUSTED_USERS_KEY: &str = "distrusted_users";

/// Nonces of invite grants already redeemed. An invite admits one device; the
/// nonce is recorded here so presenting it again is refused.
pub const INVITE_NONCES_KEY: &str = "invite_nonces";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerClaim {
    pub owner: String,
    /// hex(sign(peer_keypair, "owner:{owner}"))
    ///
    /// On its own this proves only that whoever holds this peer's key wrote the
    /// name — any peer can claim to be "alice". The fields below are what make
    /// the name mean something.
    pub sig: String,
    /// hex(protobuf) of the user identity this peer says it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_pubkey_hex: Option<String>,
    /// hex(protobuf) of the device key behind this peer's circle key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_pubkey_hex: Option<String>,
    /// hex(sign(circle_key, binding over circle id + device key)) — ties this
    /// peer id to that device. Without it the device key would be a free claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_binding_hex: Option<String>,
    /// Signatures carrying the user root's authority down to the device key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestation_chain: Vec<crate::identity::ChainLink>,
}

impl OwnerClaim {
    /// The user identity this claim *proves*, if it proves one.
    ///
    /// Returns the user public key only when all three hold: this peer's own
    /// key signed for a device key, the user root's authority reaches that
    /// device key, and the peer signed the name it is claiming. Anything
    /// missing or inconsistent returns `None` — an unproven claim is not a
    /// weaker claim, it is a display string with nothing behind it.
    ///
    /// A claim written before any of this existed returns `None` too, which is
    /// the honest answer for it.
    pub fn verified_user(&self, peer_id: &str, circle_id: &str) -> Option<String> {
        let user_pubkey = self.user_pubkey_hex.as_deref()?;
        let device_pubkey = self.device_pubkey_hex.as_deref()?;
        let binding = self.device_binding_hex.as_deref()?;

        if !crate::identity::verify_binding(peer_id, circle_id, device_pubkey, binding).ok()? {
            return None;
        }
        if !crate::identity::verify_chain(&self.attestation_chain, user_pubkey, device_pubkey)
            .ok()?
        {
            return None;
        }
        Some(user_pubkey.to_string())
    }
}

/// A user identity this circle has disowned.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DistrustEntry {
    /// hex(protobuf) of the user root public key being disowned.
    pub user_pubkey_hex: String,
    /// Whatever the admin wrote down, shown wherever the distrust is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub at: DateTime<Utc>,
    /// hex(sign(admin_key, "distrust:{user_pubkey_hex}"))
    pub admin_signature: String,
}

impl DistrustEntry {
    /// Whether this record was really signed by `admin_pubkey_hex`.
    ///
    /// The control document is replicated and every member can write to it, so
    /// the presence of an entry proves nothing — any member could add one and
    /// lock an arbitrary identity out of the circle. Only the admin signature
    /// over [`distrust_message`] makes it authority, and it is checked wherever
    /// the record is *enforced*, not where it arrives.
    pub fn is_authentic(&self, admin_pubkey_hex: &str) -> bool {
        if admin_pubkey_hex.trim().is_empty() {
            // A circle with no admin key recorded cannot check anything; refuse
            // to act on the record rather than take it on faith.
            return false;
        }
        let Ok(bytes) = hex::decode(admin_pubkey_hex.trim()) else {
            return false;
        };
        let Ok(key) = libp2p::identity::PublicKey::try_decode_protobuf(&bytes) else {
            return false;
        };
        let Ok(sig) = hex::decode(self.admin_signature.trim()) else {
            return false;
        };
        key.verify(distrust_message(&self.user_pubkey_hex).as_bytes(), &sig)
    }
}

/// The message an admin signs to disown a user identity.
pub fn distrust_message(user_pubkey_hex: &str) -> String {
    format!("distrust:{}", user_pubkey_hex.trim())
}

/// The message an admin signs to take a distrust back.
pub fn trust_message(user_pubkey_hex: &str) -> String {
    format!("trust:{}", user_pubkey_hex.trim())
}

/// The invite grant a joining device presents, carried from the invite it used.
///
/// Self-contained so it can be checked without the original invite: the
/// signature covers the circle, the nonce and the expiry recorded here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JoinGrant {
    /// hex(protobuf) of the issuing member's per-circle public key.
    pub inviter_pubkey_hex: String,
    /// One-time identifier, recorded on redemption so an invite admits once.
    pub nonce: String,
    /// hex signature over `invite:{circle_id}:{nonce}:{expires_at}`.
    pub sig: String,
    /// The expiry the signature covers.
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEntry {
    pub peer_id: String,
    pub owner: String,
    pub agent_id: String,
    /// Human-readable device label (e.g. "macbook-pro").
    #[serde(default)]
    pub device_label: String,
    /// Agent names registered on this device (e.g. ["human", "claude-code"]).
    #[serde(default)]
    pub agents: Vec<String>,
    /// hex(sign(peer_keypair, "owner:{owner}"))
    pub owner_sig: String,
    pub requested_at: DateTime<Utc>,
    /// Grant from the invite this device joined with. Absent for devices that
    /// joined before grants existed, and for invites minted without one.
    #[serde(default)]
    pub join_grant: Option<JoinGrant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlsCommitEntry {
    pub epoch: u64,
    pub data_hex: String,
    pub sender_peer_id: String,
    pub ratchet_tree_hex: String,
}

// ── Lock ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    #[serde(default)]
    pub run_id: Option<String>,
    pub entry_id: String,
    pub agent_id: String,
    /// Device that vouched for `agent_id`. Empty for legacy entries.
    #[serde(default)]
    pub peer_id: String,
    pub path: String,
    pub action: LockAction,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LockAction {
    Acquire,
    Release,
}

// ── Task ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub task_id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub created_by: String,
    /// Device that vouched for `created_by`. Empty for legacy tasks.
    #[serde(default)]
    pub created_by_peer_id: String,
    pub claimed_by: Option<String>,
    #[serde(default)]
    pub claimed_by_peer_id: Option<String>,
    #[serde(default)]
    pub unclaimed_by: Option<String>,
    #[serde(default)]
    pub unclaimed_by_peer_id: Option<String>,
    #[serde(default)]
    pub completed_by: Option<String>,
    #[serde(default)]
    pub completed_by_peer_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Open,
    Claimed,
    Done,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Claimed => write!(f, "claimed"),
            Self::Done => write!(f, "done"),
        }
    }
}

// ── Presence ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub agent_id: String,
    pub status: AgentStatus,
    pub last_seen: DateTime<Utc>,
    pub current_file: Option<String>,
    /// The peer_id of the device this agent is running on. Links presence to member entry.
    #[serde(default)]
    pub peer_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Online,
    Idle,
    Offline,
}

// ── Members ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemberRole {
    Admin,
    Member,
}

impl std::fmt::Display for MemberRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admin => write!(f, "admin"),
            Self::Member => write!(f, "member"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberEntry {
    pub peer_id: String,
    /// Human owner — groups machines belonging to the same person (e.g. "alice").
    /// Multiple peer_ids with the same owner = same user on different machines.
    pub owner: String,
    /// Primary agent label for this device (legacy display name, e.g. "alice-Kj4R").
    pub agent_id: String,
    /// Human-readable device label (e.g. "macbook-pro"). Shown in grouped UI.
    #[serde(default)]
    pub device_label: String,
    /// Agent names enrolled from this device (e.g. ["human", "claude-code"]).
    /// Pure labels — no separate keys. File edits are attributed to the device (peer_id).
    #[serde(default)]
    pub agents: Vec<String>,
    /// Which of `agents` read every human message rather than waiting to be
    /// addressed (engagement spec §2.2).
    ///
    /// Advertised so *all* peers can see who is listening, not just the device
    /// that configured it. Ambient engagement sends every human line to a model
    /// provider; the room is entitled to know that is happening, and by whom.
    /// Empty for peers predating the field, and for devices with none.
    #[serde(default)]
    pub ambient_agents: Vec<String>,
    pub role: MemberRole,
    pub added_at: DateTime<Utc>,
    /// Hex-encoded Ed25519 admin signature of "add:{peer_id}:{role}"
    pub signature: String,
}

#[cfg(test)]
mod owner_claim_tests {
    use super::*;
    use crate::identity::{sign_binding, ChainLink, DeviceIdentity, UserIdentity};

    const CIRCLE: &str = "8e563c41-f0ec-4225-9764-064f1fb04341";

    /// Build the claim a linked device publishes for a circle.
    fn claim_for(device: &DeviceIdentity, owner: &str) -> (String, OwnerClaim) {
        let circle_kp = device.derive_circle_keypair(CIRCLE).unwrap();
        let peer_id = circle_kp.public().to_peer_id().to_string();
        let device_pk = device.device_pubkey_hex().unwrap();
        let claim = OwnerClaim {
            owner: owner.into(),
            sig: hex::encode(circle_kp.sign(format!("owner:{owner}").as_bytes()).unwrap()),
            user_pubkey_hex: device.user_pubkey_hex.clone(),
            device_pubkey_hex: Some(device_pk.clone()),
            device_binding_hex: Some(
                sign_binding(&device.device_keypair().unwrap(), CIRCLE, &circle_kp).unwrap(),
            ),
            attestation_chain: device.attestation_chain.clone(),
        };
        (peer_id, claim)
    }

    fn linked_device(user: &UserIdentity, label: &str) -> DeviceIdentity {
        let mut d = DeviceIdentity::generate(label.into());
        let pk = d.device_pubkey_hex().unwrap();
        d.user_handle = Some(user.handle.clone());
        d.user_pubkey_hex = Some(user.pubkey_hex().unwrap());
        d.attestation_chain = vec![ChainLink {
            signer_pubkey_hex: user.pubkey_hex().unwrap(),
            subject_pubkey_hex: pk.clone(),
            sig: user.attest_device(&pk).unwrap(),
        }];
        d
    }

    /// A linked device's claim proves which user it belongs to.
    #[test]
    fn a_complete_claim_proves_its_user() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let device = linked_device(&user, "laptop");
        let (peer_id, claim) = claim_for(&device, "suzy");

        assert_eq!(
            claim.verified_user(&peer_id, CIRCLE).as_deref(),
            Some(user.pubkey_hex().unwrap().as_str())
        );
    }

    /// Two devices of the same person prove the *same* user key, which is what
    /// makes grouping them together meaningful rather than a guess from a
    /// matching display name.
    #[test]
    fn two_devices_of_one_user_prove_the_same_identity() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let laptop = linked_device(&user, "laptop");

        let mut phone = DeviceIdentity::generate("phone".into());
        let chain = laptop.attest(&phone.device_pubkey_hex().unwrap()).unwrap();
        phone
            .adopt_chain("suzy".into(), user.pubkey_hex().unwrap(), chain)
            .unwrap();

        let (laptop_peer, laptop_claim) = claim_for(&laptop, "suzy");
        let (phone_peer, phone_claim) = claim_for(&phone, "suzy");

        assert_eq!(
            laptop_claim.verified_user(&laptop_peer, CIRCLE),
            phone_claim.verified_user(&phone_peer, CIRCLE)
        );
        assert!(laptop_claim.verified_user(&laptop_peer, CIRCLE).is_some());
    }

    /// The whole point. Anyone could always write `owner: "suzy"`; what they
    /// cannot do is produce the signatures behind it.
    #[test]
    fn claiming_someone_elses_name_proves_nothing() {
        let (suzy, _) = UserIdentity::generate("suzy".into()).unwrap();
        let real = linked_device(&suzy, "laptop");
        let (real_peer, real_claim) = claim_for(&real, "suzy");
        let genuine = real_claim.verified_user(&real_peer, CIRCLE).unwrap();

        // An impostor with its own device, claiming the same display name.
        let impostor = DeviceIdentity::generate("impostor".into());
        let (imp_peer, imp_claim) = claim_for(&impostor, "suzy");
        assert_eq!(imp_claim.owner, "suzy");
        assert_eq!(
            imp_claim.verified_user(&imp_peer, CIRCLE),
            None,
            "an unbacked name must not verify"
        );

        // Nor by copying the real claim's proofs onto its own peer id.
        let mut stolen = real_claim.clone();
        stolen.owner = "suzy".into();
        assert_eq!(
            stolen.verified_user(&imp_peer, CIRCLE),
            None,
            "proofs must not transfer to another peer"
        );
        assert_ne!(genuine, String::new());
    }

    /// The spoof the first version of the binding allowed, end to end.
    ///
    /// Reusing the victim's binding verbatim fails on the peer id, which the
    /// test above covers. The real attack does not reuse it: everything a peer
    /// publishes about its identity is readable by every member, so an attacker
    /// takes the victim's user key, device key and chain, and mints a *fresh*
    /// binding with the one key they control. That passed, and `verified_user`
    /// returned the victim.
    #[test]
    fn a_member_cannot_mint_a_binding_to_wear_another_identity() {
        let (suzy, _) = UserIdentity::generate("suzy".into()).unwrap();
        let victim = linked_device(&suzy, "victim-laptop");
        let (victim_peer, victim_claim) = claim_for(&victim, "suzy");
        assert!(
            victim_claim.verified_user(&victim_peer, CIRCLE).is_some(),
            "the genuine claim should verify"
        );

        // An ordinary member of the circle, with their own device and circle key.
        let attacker = DeviceIdentity::generate("attacker".into());
        let attacker_circle_kp = attacker.derive_circle_keypair(CIRCLE).unwrap();
        let attacker_peer = attacker_circle_kp.public().to_peer_id().to_string();

        let forged = OwnerClaim {
            owner: "suzy".into(),
            // Signed with the attacker's own circle key — they hold it.
            sig: hex::encode(attacker_circle_kp.sign(b"owner:suzy").unwrap()),
            // Copied wholesale from what the victim published.
            user_pubkey_hex: victim_claim.user_pubkey_hex.clone(),
            device_pubkey_hex: victim_claim.device_pubkey_hex.clone(),
            attestation_chain: victim_claim.attestation_chain.clone(),
            // Minted fresh, with the only private key the attacker has.
            device_binding_hex: Some(
                crate::identity::sign_binding(
                    &attacker.device_keypair().unwrap(),
                    CIRCLE,
                    &attacker_circle_kp,
                )
                .unwrap(),
            ),
        };

        assert_eq!(
            forged.verified_user(&attacker_peer, CIRCLE),
            None,
            "an attacker was returned as the victim"
        );
    }

    /// A claim from a device that was never linked is unproven, not rejected —
    /// this is every peer today, and they must still appear.
    #[test]
    fn a_claim_without_proofs_is_simply_unverified() {
        let device = DeviceIdentity::generate("plain".into());
        let (peer_id, mut claim) = claim_for(&device, "someone");
        claim.user_pubkey_hex = None;
        claim.attestation_chain.clear();
        assert_eq!(claim.verified_user(&peer_id, CIRCLE), None);

        // And a claim written before any of this existed.
        let legacy = OwnerClaim {
            owner: "someone".into(),
            sig: String::new(),
            user_pubkey_hex: None,
            device_pubkey_hex: None,
            device_binding_hex: None,
            attestation_chain: Vec::new(),
        };
        assert_eq!(legacy.verified_user(&peer_id, CIRCLE), None);
    }

    /// A claim proven in one circle must not carry into another.
    #[test]
    fn a_claim_does_not_carry_between_circles() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let device = linked_device(&user, "laptop");
        let (peer_id, claim) = claim_for(&device, "suzy");

        assert!(claim.verified_user(&peer_id, CIRCLE).is_some());
        assert_eq!(
            claim.verified_user(&peer_id, "00000000-0000-0000-0000-000000000000"),
            None
        );
    }

    /// Old claims must still deserialise, or upgrading a peer would drop every
    /// owner name already in the control doc.
    #[test]
    fn a_claim_written_before_these_fields_still_parses() {
        let legacy = r#"{"owner":"alice","sig":"ab12"}"#;
        let claim: OwnerClaim = serde_json::from_str(legacy).unwrap();
        assert_eq!(claim.owner, "alice");
        assert!(claim.user_pubkey_hex.is_none());
        assert!(claim.attestation_chain.is_empty());
    }
}

// ── Chat ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Stable chat-thread root; independent from the ACP conversation ID.
    #[serde(default)]
    pub thread_root: Option<String>,
    pub id: String,
    pub agent_id: String,
    pub text: String,
    pub mentions: Vec<String>,
    pub ts: i64, // Unix timestamp seconds
    /// Peer that posted this message. `agent_id` alone is ambiguous for agent
    /// replies — it is the bare agent name (e.g. "codex"), which several
    /// devices may configure, so a reader cannot tell which device actually
    /// ran it. This pins the origin. Empty for messages from peers predating
    /// the field (readers fall back to name matching).
    #[serde(default)]
    pub peer_id: String,
    /// Images and other binary payloads posted with this message.
    ///
    /// Only the metadata travels in the control doc — the bytes live in the
    /// content-addressed blob store and are fetched over the sync stream. A
    /// message may carry attachments with no text.
    ///
    /// `#[serde(default)]` keeps messages written by peers predating the field
    /// parseable, and keeps stored `control.json` transcripts readable after
    /// an upgrade.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Delegation provenance: which human message this cascade is rooted in,
    /// how many agent turns it has already cost, and which agents are on this
    /// branch. `None` on messages from peers predating the field, which reads
    /// as "no allowance" — exactly the pre-delegation behaviour.
    ///
    /// This rides on the wire because the device that fires a trigger and the
    /// device that runs the mentioned agent are routinely different machines.
    /// It is a *hint that can only shrink*: the receiving device clamps it to
    /// its own configured maximum and keeps its own per-root count, so a forged
    /// chain from a hostile peer buys nothing.
    #[serde(default)]
    pub relay: Option<Relay>,
    /// Who wrote this message.
    ///
    /// Neither existing field answers this. `agent_id` is a display label — a
    /// person may legitimately be called `codex`. `peer_id` identifies the
    /// *device*, which does not separate a person from the agent running on
    /// their machine; both post from the same peer.
    ///
    /// Defaults to [`Author::Human`] for messages from peers predating the
    /// field, which is the safe reading: a human message is the one that may
    /// trigger work, and treating an old peer's agent reply as human at worst
    /// offers one unaddressed turn, where the reverse would silently stop
    /// answering people.
    #[serde(default)]
    pub author: Author,
    /// The message this one replies to, if any (engagement spec §1.4).
    ///
    /// Explicit addressing without a mention. The follow-up window (§1.1)
    /// guesses from recency and will sometimes guess wrong in a busy room;
    /// this is the form that cannot, and the only one that works when two
    /// agents are mid-conversation with the same person.
    ///
    /// Absent on messages from peers predating the field, which simply fall
    /// back to the window.
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// Authorship of a chat message. The durable signal behind "only human
/// messages trigger an unaddressed turn" (engagement spec §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    #[default]
    Human,
    Agent,
    System,
}

/// Provenance of one delegation cascade. See `docs/development/engagement.md`
/// §3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relay {
    /// Id of the human message at the base of the cascade.
    pub root: String,
    /// Peer that posted that human message. Attribution resolves from here,
    /// not from the mentioning agent — otherwise an agent could launder a
    /// remote member's request into a local one by relaying it.
    #[serde(default)]
    pub root_peer: String,
    /// Agent turns this cascade has already cost, the posting turn included.
    pub spent: u8,
    /// Agents already on this branch, innermost last.
    #[serde(default)]
    pub path: Vec<String>,
}

/// A blob referenced by a chat message. `hash` is the SHA-256 of the bytes,
/// which is both the identity and the integrity check — a receiver verifies it
/// before storing, so a peer cannot serve different content than it advertised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub hash: String,
    /// Server-sniffed content type, never the value the uploader claimed.
    pub mime: String,
    /// Original filename, for download and as the accessible fallback label.
    pub name: String,
    pub size: u64,
    /// Pixel dimensions when the blob is a decodable image. Used to reserve
    /// layout space before the bytes arrive, so the transcript does not jump.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// A tombstone marking a path as deleted.
///
/// Deliberately **not** permanent, unlike the member-removal tombstone: files
/// are routinely deleted and re-created under the same name. Re-creating a path
/// clears its tombstone, and `ts` exists so a stale tombstone arriving late
/// cannot delete a file that was re-created after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deletion {
    pub path: String,
    /// Unix milliseconds. Milliseconds rather than seconds because a delete and
    /// a re-create of the same path routinely land inside the same second.
    pub ts: i64,
    /// Device that performed the deletion, for attribution in the UI.
    #[serde(default)]
    pub peer_id: String,
}

/// A short-lived, non-transcript signal shown alongside chat. `activity_id`
/// identifies one producer/run, while `expires_at` makes stale indicators
/// disappear even when a peer disconnects before it can clear them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatActivity {
    pub activity_id: String,
    pub actor_id: String,
    #[serde(default)]
    pub peer_id: String,
    pub kind: ChatActivityKind,
    /// Why, for kinds that need saying — currently only [`ChatActivityKind::Skipped`].
    /// Absent on activities from peers predating the field.
    #[serde(default)]
    pub detail: Option<String>,
    pub message_id: Option<String>,
    pub updated_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatActivityKind {
    Typing,
    Seen,
    Working,
    /// The device had an agent for this mention and chose not to run it — a
    /// delegation cascade out of budget, or one a person stopped.
    ///
    /// Silence must be legible: an agent that was considered and declined is
    /// not the same as an adapter that crashed, and a user who cannot tell
    /// them apart stops trusting the Circle. This is deliberately *not* a
    /// `system` chat post — a permanent transcript line per dead mention is
    /// exactly the noise a busy cascade would drown the room in.
    Skipped,
}

// ── Events ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CircleEvent {
    FileUpdated {
        path: String,
    },
    FileDeleted {
        path: String,
    },
    LockAcquired {
        path: String,
        agent_id: String,
    },
    LockReleased {
        path: String,
        agent_id: String,
    },
    TaskCreated {
        task_id: String,
    },
    TaskClaimed {
        task_id: String,
        agent_id: String,
    },
    TaskUnclaimed {
        task_id: String,
        agent_id: String,
    },
    TaskDone {
        task_id: String,
    },
    PresenceChanged {
        agent_id: String,
    },
    MemberAdded {
        peer_id: String,
    },
    MemberRemoved {
        peer_id: String,
    },
    /// A join request appeared in, or was cleared from, the pending map.
    /// Carries only the peer — viewers refetch the roster and pending list.
    MemberPending {
        peer_id: String,
    },
    /// A chat message was posted to the circle.
    MessagePosted {
        message: ChatMessage,
    },
    /// A message mentioned a specific agent — the agent's wake signal.
    AgentMentioned {
        agent_id: String,
        message: ChatMessage,
    },
    /// Attachment bytes finished downloading from a peer. Viewers showing a
    /// placeholder for this hash can now load it.
    AttachmentAvailable {
        hash: String,
    },
    /// Ephemeral typing / agent lifecycle state. This is never a chat message.
    /// Someone halted a delegation cascade; no further relayed turns run for
    /// this root on any device.
    RelayStopped {
        root: String,
    },
    /// A peer dismissed its follow-up window.
    EngagementChanged {
        peer_id: String,
    },
    ChatActivityChanged {
        activity: ChatActivity,
    },
    /// The proposal engine captured a workspace change (M14).
    ProposalCreated {
        proposal_id: String,
    },
    /// A proposal's status changed (accepted / rejected / reverted).
    ProposalUpdated {
        proposal_id: String,
        status: String,
    },
    /// An immutable M15 workspace event was appended locally or received.
    WorkspaceEventAppended {
        event_id: String,
    },
}

#[cfg(test)]
mod distrust_tests {
    use super::*;
    use crate::identity::{sign_binding, ChainLink, DeviceIdentity, UserIdentity};

    const CIRCLE: &str = "8e563c41-f0ec-4225-9764-064f1fb04341";

    fn linked_device(user: &UserIdentity, label: &str) -> DeviceIdentity {
        let mut d = DeviceIdentity::generate(label.into());
        let pk = d.device_pubkey_hex().unwrap();
        d.user_handle = Some(user.handle.clone());
        d.user_pubkey_hex = Some(user.pubkey_hex().unwrap());
        d.attestation_chain = vec![ChainLink {
            signer_pubkey_hex: user.pubkey_hex().unwrap(),
            subject_pubkey_hex: pk.clone(),
            sig: user.attest_device(&pk).unwrap(),
        }];
        d
    }

    fn claim_for(device: &DeviceIdentity) -> (String, OwnerClaim) {
        let circle_kp = device.derive_circle_keypair(CIRCLE).unwrap();
        let peer_id = circle_kp.public().to_peer_id().to_string();
        let claim = OwnerClaim {
            owner: device.user_handle.clone().unwrap_or_default(),
            sig: String::new(),
            user_pubkey_hex: device.user_pubkey_hex.clone(),
            device_pubkey_hex: Some(device.device_pubkey_hex().unwrap()),
            device_binding_hex: Some(
                sign_binding(&device.device_keypair().unwrap(), CIRCLE, &circle_kp).unwrap(),
            ),
            attestation_chain: device.attestation_chain.clone(),
        };
        (peer_id, claim)
    }

    /// The reason distrust is keyed on the identity rather than the device.
    ///
    /// Whoever holds a stolen root key can mint devices without limit, so
    /// removing them one peer at a time never finishes. Every device proving the
    /// same identity — including one made after the distrust — resolves to the
    /// same key, which is the thing a circle can actually refuse.
    #[test]
    fn every_device_of_an_identity_resolves_to_the_one_key() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let laptop = linked_device(&user, "laptop");

        // A device minted after the fact, as an attacker with the root key would.
        let later = linked_device(&user, "minted-later");

        let (laptop_peer, laptop_claim) = claim_for(&laptop);
        let (later_peer, later_claim) = claim_for(&later);

        let a = laptop_claim.verified_user(&laptop_peer, CIRCLE).unwrap();
        let b = later_claim.verified_user(&later_peer, CIRCLE).unwrap();
        assert_eq!(a, b, "devices of one identity must resolve to one key");
        assert_eq!(a, user.pubkey_hex().unwrap());
    }

    /// A different identity must not be caught by someone else's distrust.
    #[test]
    fn another_identity_is_not_covered() {
        let (suzy, _) = UserIdentity::generate("suzy".into()).unwrap();
        let (mallory, _) = UserIdentity::generate("mallory".into()).unwrap();

        let (p1, c1) = claim_for(&linked_device(&suzy, "laptop"));
        let (p2, c2) = claim_for(&linked_device(&mallory, "laptop"));

        assert_ne!(
            c1.verified_user(&p1, CIRCLE).unwrap(),
            c2.verified_user(&p2, CIRCLE).unwrap()
        );
    }

    /// The signed message covers the resolved user key, so a record can be
    /// checked later against the key it names rather than whatever was typed.
    #[test]
    fn the_signed_messages_name_the_user_key() {
        let key = "0801122000112233";
        assert_eq!(distrust_message(key), "distrust:0801122000112233");
        assert_eq!(trust_message(key), "trust:0801122000112233");
        // Distrust and its undo must never be the same bytes, or one signature
        // would serve for both.
        assert_ne!(distrust_message(key), trust_message(key));
        // Whitespace is normalised, so a copied-in key signs the same string.
        assert_eq!(
            distrust_message("  0801122000112233 "),
            distrust_message(key)
        );
    }

    /// A distrust record round-trips, and carries who said so.
    #[test]
    fn a_distrust_record_round_trips() {
        let entry = DistrustEntry {
            user_pubkey_hex: "0801122000112233".into(),
            reason: Some("laptop stolen".into()),
            at: Utc::now(),
            admin_signature: "ab12".into(),
        };
        let back: DistrustEntry =
            serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(back, entry);
    }

    /// The control document is replicated and every member can write to it, so
    /// an entry that merely exists proves nothing. Without the signature check,
    /// any member could add one and lock an arbitrary identity out of the
    /// circle — a denial of service available to everybody inside it.
    #[test]
    fn a_distrust_record_is_only_authority_when_the_admin_signed_it() {
        use libp2p::identity::Keypair;

        let admin = Keypair::generate_ed25519();
        let admin_hex = hex::encode(admin.public().encode_protobuf());
        let target = "0801122000112233";

        let genuine = DistrustEntry {
            user_pubkey_hex: target.into(),
            reason: None,
            at: Utc::now(),
            admin_signature: hex::encode(admin.sign(distrust_message(target).as_bytes()).unwrap()),
        };
        assert!(genuine.is_authentic(&admin_hex));

        // What a member could write into the map on their own.
        let forged = DistrustEntry {
            admin_signature: String::new(),
            ..genuine.clone()
        };
        assert!(
            !forged.is_authentic(&admin_hex),
            "an unsigned record was taken as authority"
        );

        // Signed by somebody who is not the admin.
        let impostor = Keypair::generate_ed25519();
        let by_impostor = DistrustEntry {
            admin_signature: hex::encode(
                impostor.sign(distrust_message(target).as_bytes()).unwrap(),
            ),
            ..genuine.clone()
        };
        assert!(!by_impostor.is_authentic(&admin_hex));

        // A real signature, moved onto a different identity.
        let moved = DistrustEntry {
            user_pubkey_hex: "0801122099887766".into(),
            ..genuine.clone()
        };
        assert!(
            !moved.is_authentic(&admin_hex),
            "a signature was reused for another identity"
        );

        // And a circle with no admin key recorded cannot check anything, so it
        // must refuse rather than take the record on faith.
        assert!(!genuine.is_authentic(""));
        assert!(!genuine.is_authentic("not hex"));
    }

    /// `trust` must not undo by accident: a signature over the undo message is
    /// not a distrust, or one could be replayed as the other.
    #[test]
    fn a_trust_signature_does_not_authenticate_a_distrust() {
        use libp2p::identity::Keypair;

        let admin = Keypair::generate_ed25519();
        let admin_hex = hex::encode(admin.public().encode_protobuf());
        let target = "0801122000112233";

        let entry = DistrustEntry {
            user_pubkey_hex: target.into(),
            reason: None,
            at: Utc::now(),
            admin_signature: hex::encode(admin.sign(trust_message(target).as_bytes()).unwrap()),
        };
        assert!(!entry.is_authentic(&admin_hex));
    }

    /// A peer that proves nothing has no identity to distrust — the record is
    /// keyed on a user key, and an unproven display name is not one.
    #[test]
    fn a_peer_that_proves_nothing_has_no_identity_to_distrust() {
        let plain = DeviceIdentity::generate("unlinked".into());
        let (peer, claim) = claim_for(&plain);
        assert_eq!(claim.verified_user(&peer, CIRCLE), None);
    }
}
