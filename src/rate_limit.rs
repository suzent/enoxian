//! A small per-source request budget, for the endpoints the bootstrap server
//! exposes to strangers.
//!
//! Both of them — the pairing mailbox and the invite blob store — hand out
//! nothing an attacker can use, so this is not what makes either of them safe.
//! It is here so neither can be used to hammer the host, and to take some
//! margin back from someone spreading requests across addresses.
//!
//! Shared rather than written twice: the table needs a bound of its own, or the
//! limiter becomes the thing it was added to prevent, and that is a mistake
//! worth making in only one place.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// What a budget is counted against.
///
/// A single IPv6 address means nothing — one host commonly controls a whole /64
/// — so v6 is bucketed by that prefix. v4 is counted per address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RateKey {
    V4([u8; 4]),
    V6Prefix([u8; 8]),
}

impl From<IpAddr> for RateKey {
    fn from(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(v4) => RateKey::V4(v4.octets()),
            IpAddr::V6(v6) => {
                let o = v6.octets();
                RateKey::V6Prefix(o[..8].try_into().expect("an IPv6 address has 16 octets"))
            }
        }
    }
}

/// Fixed-window request counting, bounded in both time and size.
pub struct Limiter {
    seen: HashMap<RateKey, (u32, Instant)>,
    max_per_window: u32,
    window: Duration,
    max_tracked: usize,
}

impl Limiter {
    /// `max_per_window` requests per `window` from one source, tracking at most
    /// `max_tracked` sources at a time.
    pub fn new(max_per_window: u32, window: Duration, max_tracked: usize) -> Self {
        Limiter {
            seen: HashMap::new(),
            max_per_window,
            window,
            max_tracked,
        }
    }

    /// Drop entries whose window has passed. Call on each request, so the table
    /// stays bounded without a background task to supervise.
    pub fn expire(&mut self) {
        let now = Instant::now();
        self.seen
            .retain(|_, (_, started)| now.duration_since(*started) < self.window);
    }

    /// Charge one request against a source's budget. `false` means refuse.
    pub fn allow(&mut self, key: RateKey) -> bool {
        let now = Instant::now();
        // A source already being tracked is always charged; only a new one can
        // be turned away for want of room, so the table cannot be grown without
        // limit by arriving from ever more addresses. Fail closed, like the
        // stores' own caps.
        if !self.seen.contains_key(&key) && self.seen.len() >= self.max_tracked {
            return false;
        }
        let entry = self.seen.entry(key).or_insert((0, now));
        if now.duration_since(entry.1) >= self.window {
            *entry = (0, now);
        }
        entry.0 += 1;
        entry.0 <= self.max_per_window
    }

    /// Sources currently tracked. For tests and diagnostics.
    pub fn tracked(&self) -> usize {
        self.seen.len()
    }

    /// Age every entry out of its window. For tests.
    #[cfg(test)]
    pub fn force_expire_all(&mut self) {
        let past = Instant::now() - self.window - Duration::from_secs(1);
        for entry in self.seen.values_mut() {
            entry.1 = past;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(n: u8) -> RateKey {
        RateKey::from(IpAddr::from([10, 0, 0, n]))
    }

    fn seq(i: usize) -> RateKey {
        let [_, b, c, d] = (i as u32).to_be_bytes();
        RateKey::from(IpAddr::from([10, b, c, d]))
    }

    fn limiter() -> Limiter {
        Limiter::new(5, Duration::from_secs(10), 16)
    }

    #[test]
    fn a_source_is_refused_once_it_runs_out_of_budget() {
        let mut l = limiter();
        for i in 0..5 {
            assert!(l.allow(v4(1)), "refused at {i}");
        }
        assert!(!l.allow(v4(1)));
    }

    /// One noisy source must not spend another's budget, or a single bad actor
    /// stops everyone else.
    #[test]
    fn budgets_are_per_source() {
        let mut l = limiter();
        for _ in 0..10 {
            l.allow(v4(1));
        }
        assert!(l.allow(v4(2)));
    }

    /// The window must roll, or a source is locked out for as long as the
    /// process lives.
    #[test]
    fn the_budget_refills_after_the_window() {
        let mut l = limiter();
        for _ in 0..6 {
            l.allow(v4(1));
        }
        assert!(!l.allow(v4(1)));
        l.force_expire_all();
        assert!(l.allow(v4(1)));
    }

    /// The table needs a bound of its own, or the limiter becomes unbounded
    /// growth — trivially so over IPv6, where each /64 is a separate key.
    #[test]
    fn the_table_cannot_be_grown_without_limit() {
        let mut l = limiter();
        for i in 0..16 {
            l.allow(seq(i));
        }
        assert_eq!(l.tracked(), 16);
        assert!(!l.allow(seq(9999)), "a new source should be turned away");
        assert_eq!(l.tracked(), 16);
    }

    /// A source already tracked keeps working when the table is full, so a
    /// flood of new addresses cannot push live traffic out.
    #[test]
    fn a_tracked_source_survives_a_flood_of_new_ones() {
        let mut l = limiter();
        l.allow(seq(0));
        for i in 1..100 {
            l.allow(seq(i));
        }
        assert!(l.allow(seq(0)));
    }

    #[test]
    fn expiring_releases_capacity() {
        let mut l = limiter();
        for i in 0..16 {
            l.allow(seq(i));
        }
        assert!(!l.allow(seq(9999)));
        l.force_expire_all();
        l.expire();
        assert_eq!(l.tracked(), 0);
        assert!(l.allow(seq(9999)));
    }

    /// A whole IPv6 /64 is usually one attacker, so it shares one budget —
    /// otherwise the limit is free to walk around.
    #[test]
    fn an_ipv6_prefix_shares_one_budget() {
        let a: IpAddr = "2001:db8::1".parse().unwrap();
        let b: IpAddr = "2001:db8::dead:beef".parse().unwrap();
        let elsewhere: IpAddr = "2001:db9::1".parse().unwrap();

        assert_eq!(RateKey::from(a), RateKey::from(b), "same /64");
        assert_ne!(RateKey::from(a), RateKey::from(elsewhere), "different /64");
    }
}
