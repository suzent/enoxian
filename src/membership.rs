//! Admin-signed, versioned membership standing.
//!
//! A removal used to be a bare tombstone in [`MLS_REMOVED_KEY`]: a timestamp
//! nobody signed and nothing ever cleared. A device's per-circle peer id is
//! derived from its device key, so a removed device that was invited back
//! returned under the same id, still matched its tombstone, and was refused
//! forever (#260).
//!
//! The fix is a record per change of standing, `{peer_id, in|out, gen}`,
//! signed by the circle's admin key. The record with the highest `gen` for a
//! peer decides; at equal `gen`, `out` wins. Nothing is compared by wall-clock
//! time, so neither clock skew nor the order records arrive in matters, and a
//! remove → readmit → remove → readmit sequence of any length resolves the
//! same way on every device:
//!
//! - an old record cannot be replayed to undo a newer one, because it loses on
//!   `gen`;
//! - a removed device cannot raise the `gen` of a record it holds, because
//!   `gen` is under the signature;
//! - and a record need not be trusted for where it came from, so any peer may
//!   relay one, including the removed device itself.
//!
//! Records are stored under `{peer_id}#{gen}#{in|out}`, never overwritten:
//! two devices relaying different records for one peer then write different
//! keys instead of racing on one, which a CRDT map would settle by client id
//! rather than by `gen`.
//!
//! A peer with no valid record falls back to the legacy tombstone, so a
//! removal made before this existed still holds until the admin readmits.

use crate::control::{MEMBERSHIP_KEY, MLS_REMOVED_KEY};
use serde::{Deserialize, Serialize};
use yrs::{Any, Map, Out, ReadTxn, TransactionMut, WriteTxn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    In,
    Out,
}

impl Standing {
    fn as_str(self) -> &'static str {
        match self {
            Standing::In => "in",
            Standing::Out => "out",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipRecord {
    pub peer_id: String,
    pub standing: Standing,
    pub gen: u64,
    /// When the admin wrote it. Shown to people; never used to order records.
    pub at: chrono::DateTime<chrono::Utc>,
    /// hex(sign(admin_key, [`message`]))
    pub admin_signature: String,
}

/// What the admin signs. The circle id keeps a record from one circle from
/// being replayed into another that happens to share the admin key.
pub fn message(circle_id: &str, peer_id: &str, standing: Standing, gen: u64) -> String {
    format!(
        "membership:{circle_id}:{peer_id}:{}:{gen}",
        standing.as_str()
    )
}

impl MembershipRecord {
    pub fn key(&self) -> String {
        format!("{}#{}#{}", self.peer_id, self.gen, self.standing.as_str())
    }

    /// Whether the admin really signed this record for this circle.
    pub fn is_authentic(&self, circle_id: &str, admin_pubkey_hex: &str) -> bool {
        let Ok(bytes) = hex::decode(admin_pubkey_hex.trim()) else {
            return false;
        };
        let Ok(key) = libp2p_identity::PublicKey::try_decode_protobuf(&bytes) else {
            return false;
        };
        let Ok(sig) = hex::decode(self.admin_signature.trim()) else {
            return false;
        };
        key.verify(
            message(circle_id, &self.peer_id, self.standing, self.gen).as_bytes(),
            &sig,
        )
    }
}

/// Of `records`, the one that decides: highest `gen`, and `out` over `in` at
/// the same `gen`, so an ambiguous tie keeps a device out rather than in.
pub fn deciding<'a>(
    records: impl IntoIterator<Item = &'a MembershipRecord>,
) -> Option<&'a MembershipRecord> {
    records
        .into_iter()
        .max_by_key(|r| (r.gen, r.standing == Standing::Out))
}

/// Every authentic record about `peer_id`.
fn authentic_records<T: ReadTxn>(
    txn: &T,
    circle_id: &str,
    admin_pubkey_hex: &str,
    peer_id: &str,
) -> Vec<MembershipRecord> {
    let Some(map) = txn.get_map(MEMBERSHIP_KEY) else {
        return Vec::new();
    };
    let prefix = format!("{peer_id}#");
    map.iter(txn)
        .filter(|(key, _)| key.starts_with(&prefix))
        .filter_map(|(key, value)| match value {
            Out::Any(Any::String(s)) => serde_json::from_str::<MembershipRecord>(&s)
                .ok()
                .filter(|r| r.key() == key),
            _ => None,
        })
        .filter(|r| r.peer_id == peer_id && r.is_authentic(circle_id, admin_pubkey_hex))
        .collect()
}

/// The admin's latest word on `peer_id`, if it has signed one.
pub fn standing<T: ReadTxn>(
    txn: &T,
    circle_id: &str,
    admin_pubkey_hex: &str,
    peer_id: &str,
) -> Option<Standing> {
    let records = authentic_records(txn, circle_id, admin_pubkey_hex, peer_id);
    deciding(&records).map(|r| r.standing)
}

/// Whether `peer_id` is removed from the circle: by its latest signed record,
/// or, if it has none, by the legacy tombstone.
pub fn is_removed<T: ReadTxn>(
    txn: &T,
    circle_id: &str,
    admin_pubkey_hex: &str,
    peer_id: &str,
) -> bool {
    match standing(txn, circle_id, admin_pubkey_hex, peer_id) {
        Some(standing) => standing == Standing::Out,
        None => txn
            .get_map(MLS_REMOVED_KEY)
            .and_then(|removed| removed.get(txn, peer_id))
            .is_some_and(|v| matches!(v, Out::Any(Any::String(_)))),
    }
}

/// The `gen` the admin's next record about `peer_id` takes.
pub fn next_gen<T: ReadTxn>(
    txn: &T,
    circle_id: &str,
    admin_pubkey_hex: &str,
    peer_id: &str,
) -> u64 {
    authentic_records(txn, circle_id, admin_pubkey_hex, peer_id)
        .iter()
        .map(|r| r.gen)
        .max()
        .unwrap_or(0)
        + 1
}

/// Sign and store a new record about `peer_id`, inside the caller's
/// transaction. `sign` turns the message into a hex signature by the admin
/// key.
pub fn write(
    txn: &mut TransactionMut,
    circle_id: &str,
    admin_pubkey_hex: &str,
    peer_id: &str,
    standing: Standing,
    sign: impl FnOnce(&[u8]) -> anyhow::Result<String>,
) -> anyhow::Result<MembershipRecord> {
    let record = sign_next(txn, circle_id, admin_pubkey_hex, peer_id, standing, sign)?;
    store(txn, &record)?;
    Ok(record)
}

/// The next record about `peer_id`, signed but not yet stored, so a caller
/// can make sure of the signature before doing anything it cannot undo.
pub fn sign_next<T: ReadTxn>(
    txn: &T,
    circle_id: &str,
    admin_pubkey_hex: &str,
    peer_id: &str,
    standing: Standing,
    sign: impl FnOnce(&[u8]) -> anyhow::Result<String>,
) -> anyhow::Result<MembershipRecord> {
    let gen = next_gen(txn, circle_id, admin_pubkey_hex, peer_id);
    let admin_signature = sign(message(circle_id, peer_id, standing, gen).as_bytes())?;
    let record = MembershipRecord {
        peer_id: peer_id.to_string(),
        standing,
        gen,
        at: chrono::Utc::now(),
        admin_signature,
    };
    anyhow::ensure!(
        record.is_authentic(circle_id, admin_pubkey_hex),
        "the membership record is not signed by this circle's admin key"
    );
    Ok(record)
}

/// Store a record made by [`sign_next`].
pub fn store(txn: &mut TransactionMut, record: &MembershipRecord) -> anyhow::Result<()> {
    let json = serde_json::to_string(record)?;
    let map = txn.get_or_insert_map(MEMBERSHIP_KEY);
    map.insert(txn, record.key().as_str(), json.as_str());
    Ok(())
}

/// The `gen` of the record readmitting `peer_id`, if its latest word is a
/// readmission.
pub fn readmission_gen<T: ReadTxn>(
    txn: &T,
    circle_id: &str,
    admin_pubkey_hex: &str,
    peer_id: &str,
) -> Option<u64> {
    let records = authentic_records(txn, circle_id, admin_pubkey_hex, peer_id);
    deciding(&records)
        .filter(|r| r.standing == Standing::In)
        .map(|r| r.gen)
}

/// Every stored record, as `(key, json)` pairs, for relaying.
pub fn entries<T: ReadTxn>(txn: &T) -> Vec<(String, String)> {
    let Some(map) = txn.get_map(MEMBERSHIP_KEY) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, String)> = map
        .iter(txn)
        .filter_map(|(key, value)| match value {
            Out::Any(Any::String(s)) => Some((key.to_string(), s.to_string())),
            _ => None,
        })
        .collect();
    entries.sort();
    entries
}

/// Of relayed `(key, json)` pairs, the ones worth storing: well formed, filed
/// under their own key, and signed by the admin. Who relayed them does not
/// matter, which is what lets a removed device carry its own readmission.
pub fn accept_relayed(
    circle_id: &str,
    admin_pubkey_hex: &str,
    entries: &[(String, String)],
) -> Vec<(String, String)> {
    entries
        .iter()
        .filter(|(key, json)| {
            serde_json::from_str::<MembershipRecord>(json)
                .is_ok_and(|r| r.key() == *key && r.is_authentic(circle_id, admin_pubkey_hex))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p_identity::Keypair;
    use yrs::{Doc, Transact};

    const CIRCLE: &str = "circle";

    fn admin() -> (Keypair, String) {
        let key = Keypair::generate_ed25519();
        let public = hex::encode(key.public().encode_protobuf());
        (key, public)
    }

    fn signer(key: &Keypair) -> impl Fn(&[u8]) -> anyhow::Result<String> + '_ {
        |msg| Ok(hex::encode(key.sign(msg)?))
    }

    fn record(key: &Keypair, peer: &str, standing: Standing, gen: u64) -> MembershipRecord {
        MembershipRecord {
            peer_id: peer.into(),
            standing,
            gen,
            at: chrono::Utc::now(),
            admin_signature: hex::encode(
                key.sign(message(CIRCLE, peer, standing, gen).as_bytes())
                    .unwrap(),
            ),
        }
    }

    fn store(doc: &Doc, records: &[MembershipRecord]) {
        let mut txn = doc.transact_mut();
        let map = txn.get_or_insert_map(MEMBERSHIP_KEY);
        for r in records {
            map.insert(
                &mut txn,
                r.key().as_str(),
                serde_json::to_string(r).unwrap().as_str(),
            );
        }
    }

    fn tombstone(doc: &Doc, peer: &str) {
        let mut txn = doc.transact_mut();
        let map = txn.get_or_insert_map(MLS_REMOVED_KEY);
        map.insert(&mut txn, peer, "2026-10-01T00:00:00Z");
    }

    fn removed(doc: &Doc, admin: &str, peer: &str) -> bool {
        is_removed(&doc.transact(), CIRCLE, admin, peer)
    }

    #[test]
    fn any_number_of_removals_and_readmissions_resolve_by_gen() {
        let (key, admin) = admin();
        let doc = Doc::new();
        let sign = signer(&key);
        let expected = [true, false, true, false, true, false];
        for (i, want_removed) in expected.into_iter().enumerate() {
            let standing = if want_removed {
                Standing::Out
            } else {
                Standing::In
            };
            let written = write(
                &mut doc.transact_mut(),
                CIRCLE,
                &admin,
                "peer",
                standing,
                &sign,
            )
            .unwrap();
            assert_eq!(written.gen, i as u64 + 1);
            assert_eq!(removed(&doc, &admin, "peer"), want_removed, "after {i}");
        }
    }

    #[test]
    fn the_order_records_arrive_in_does_not_matter() {
        let (key, admin) = admin();
        let history = [
            record(&key, "peer", Standing::Out, 1),
            record(&key, "peer", Standing::In, 2),
            record(&key, "peer", Standing::Out, 3),
            record(&key, "peer", Standing::In, 4),
        ];
        for order in [[0, 1, 2, 3], [3, 2, 1, 0], [2, 3, 0, 1], [3, 0, 2, 1]] {
            let doc = Doc::new();
            for i in order {
                store(&doc, std::slice::from_ref(&history[i]));
            }
            assert!(!removed(&doc, &admin, "peer"), "order {order:?}");
        }
    }

    #[test]
    fn readmission_overrides_a_legacy_tombstone_and_a_later_removal_restores_it() {
        let (key, admin) = admin();
        let doc = Doc::new();
        tombstone(&doc, "peer");
        assert!(removed(&doc, &admin, "peer"), "the legacy tombstone holds");

        store(&doc, &[record(&key, "peer", Standing::In, 1)]);
        assert!(!removed(&doc, &admin, "peer"), "readmitted");

        store(&doc, &[record(&key, "peer", Standing::Out, 2)]);
        assert!(removed(&doc, &admin, "peer"), "removed again");
    }

    #[test]
    fn a_replayed_older_readmission_does_not_undo_a_newer_removal() {
        let (key, admin) = admin();
        let doc = Doc::new();
        let readmit = record(&key, "peer", Standing::In, 2);
        store(
            &doc,
            &[readmit.clone(), record(&key, "peer", Standing::Out, 3)],
        );
        store(&doc, &[readmit]);
        assert!(removed(&doc, &admin, "peer"));
    }

    #[test]
    fn a_record_with_its_gen_raised_is_refused() {
        // The removed device holds its own old readmission and bumps the gen
        // past the removal. The signature covers the gen, so it fails.
        let (key, admin) = admin();
        let doc = Doc::new();
        store(&doc, &[record(&key, "peer", Standing::Out, 3)]);
        let mut forged = record(&key, "peer", Standing::In, 2);
        forged.gen = 4;
        store(&doc, &[forged.clone()]);
        assert!(removed(&doc, &admin, "peer"));
        assert!(accept_relayed(
            CIRCLE,
            &admin,
            &[(forged.key(), serde_json::to_string(&forged).unwrap())]
        )
        .is_empty());
    }

    #[test]
    fn a_self_signed_readmission_is_refused() {
        let (_, admin) = admin();
        let (stranger, _) = self::admin();
        let doc = Doc::new();
        tombstone(&doc, "peer");
        let forged = record(&stranger, "peer", Standing::In, 9);
        store(&doc, std::slice::from_ref(&forged));
        assert!(removed(&doc, &admin, "peer"));
        assert!(accept_relayed(
            CIRCLE,
            &admin,
            &[(forged.key(), serde_json::to_string(&forged).unwrap())]
        )
        .is_empty());
    }

    #[test]
    fn a_record_for_another_peer_or_circle_or_key_is_refused() {
        let (key, admin) = admin();
        let doc = Doc::new();
        tombstone(&doc, "peer");
        // Signed for "other", filed under "peer".
        let mut moved = record(&key, "other", Standing::In, 1);
        let json = serde_json::to_string(&moved).unwrap();
        {
            let mut txn = doc.transact_mut();
            let map = txn.get_or_insert_map(MEMBERSHIP_KEY);
            map.insert(&mut txn, "peer#1#in", json.as_str());
        }
        assert!(removed(&doc, &admin, "peer"), "filed under the wrong key");
        // Relabelled as "peer": the signature no longer matches.
        moved.peer_id = "peer".into();
        store(&doc, &[moved]);
        assert!(removed(&doc, &admin, "peer"), "signed for another peer");
        // Signed for another circle.
        let mut elsewhere = record(&key, "peer", Standing::In, 2);
        elsewhere.admin_signature = hex::encode(
            key.sign(message("another", "peer", Standing::In, 2).as_bytes())
                .unwrap(),
        );
        store(&doc, &[elsewhere]);
        assert!(removed(&doc, &admin, "peer"), "signed for another circle");
    }

    #[test]
    fn a_tie_at_the_same_gen_keeps_the_device_out() {
        // Two records can share a gen only if two holders of the admin key
        // wrote at once. Both are kept, under different keys, and out wins.
        let (key, admin) = admin();
        let doc = Doc::new();
        store(
            &doc,
            &[
                record(&key, "peer", Standing::In, 2),
                record(&key, "peer", Standing::Out, 2),
            ],
        );
        assert!(removed(&doc, &admin, "peer"));
    }

    #[test]
    fn writing_requires_the_admin_key() {
        let (_, admin) = admin();
        let (stranger, _) = self::admin();
        let doc = Doc::new();
        let result = write(
            &mut doc.transact_mut(),
            CIRCLE,
            &admin,
            "peer",
            Standing::In,
            signer(&stranger),
        );
        assert!(result.is_err());
        assert!(entries(&doc.transact()).is_empty());
    }
}
