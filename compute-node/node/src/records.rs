//! The phone records that compute nodes copy to each other: which phones are allowed, which are blocked, and the
//! tombstones that say a phone was revoked or unblocked. Only phones and times, never anything about a job.
//!
//! Each phone has two registers. One holds Allow or Revoke, the other Block or Unblock. In a register the later
//! record wins, and when two carry the same time Revoke beats Allow and Block beats Unblock, so a tie always falls on
//! the safe side. A phone is let in when it is allowed and not blocked. That makes a block beat an allow for the
//! same phone, whatever the order they arrive in, until an unblock that is later than the block exists.
//!
//! Merging takes the winner of each register, so it gives the same result in any order, any number of times.

use marginalia_proto::{PhoneRecord, RecordKind};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Register {
    Allow,
    Block,
}

fn register(k: RecordKind) -> Register {
    match k {
        RecordKind::Allow | RecordKind::Revoke => Register::Allow,
        RecordKind::Block | RecordKind::Unblock => Register::Block,
    }
}

/// The side a tie falls on: the record that takes access away.
fn strength(k: RecordKind) -> u8 {
    match k {
        RecordKind::Revoke | RecordKind::Block => 1,
        RecordKind::Allow | RecordKind::Unblock => 0,
    }
}

/// Larger wins. The name is last so that two records that differ only in it still have one winner.
fn order(r: &PhoneRecord) -> (u64, u8, &str) {
    (r.at_ms, strength(r.kind), r.name.as_str())
}

/// A fingerprint is 64 hex characters. Anything else from a peer is ignored.
fn valid(r: &PhoneRecord) -> bool {
    r.fp.len() == 64 && r.fp.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Records {
    regs: BTreeMap<(String, Register), PhoneRecord>,
}

impl Records {
    pub fn from_list(list: impl IntoIterator<Item = PhoneRecord>) -> Records {
        let mut r = Records::default();
        for rec in list {
            r.put(rec);
        }
        r
    }

    /// Keeps `rec` when it beats what is held for that register. Returns whether it was kept.
    pub fn put(&mut self, mut rec: PhoneRecord) -> bool {
        if !valid(&rec) {
            return false;
        }
        rec.name = rec.name.chars().filter(|c| !c.is_control()).take(40).collect();
        let key = (rec.fp.clone(), register(rec.kind));
        match self.regs.get(&key) {
            Some(old) if order(old) >= order(&rec) => false,
            _ => {
                self.regs.insert(key, rec);
                true
            }
        }
    }

    /// Takes in what a peer sent. Returns the records that changed anything here.
    pub fn merge(&mut self, theirs: &[PhoneRecord]) -> Vec<PhoneRecord> {
        theirs.iter().filter(|r| self.put((*r).clone())).cloned().collect()
    }

    pub fn all(&self) -> Vec<PhoneRecord> {
        self.regs.values().cloned().collect()
    }

    pub fn is_blocked(&self, fp: &str) -> bool {
        self.regs.get(&(fp.to_string(), Register::Block)).map(|r| r.kind == RecordKind::Block).unwrap_or(false)
    }

    /// The allow record of a phone that is let in: allowed and not blocked.
    pub fn allow_record(&self, fp: &str) -> Option<&PhoneRecord> {
        if self.is_blocked(fp) {
            return None;
        }
        self.regs.get(&(fp.to_string(), Register::Allow)).filter(|r| r.kind == RecordKind::Allow)
    }

    #[cfg(test)]
    pub fn is_allowed(&self, fp: &str) -> bool {
        self.allow_record(fp).is_some()
    }

    /// The allow records of phones that are let in.
    pub fn allowed(&self) -> Vec<&PhoneRecord> {
        self.regs.iter().filter(|((fp, reg), r)| *reg == Register::Allow && r.kind == RecordKind::Allow && !self.is_blocked(fp)).map(|(_, r)| r).collect()
    }

    pub fn blocked(&self) -> Vec<&PhoneRecord> {
        self.regs.iter().filter(|((_, reg), r)| *reg == Register::Block && r.kind == RecordKind::Block).map(|(_, r)| r).collect()
    }

    /// The time for a new record about `fp`: now, but never before what is already known about that phone, so a
    /// change made here always beats the one it follows even when this clock runs behind.
    pub fn next_at(&self, fp: &str, now_ms: u64) -> u64 {
        let known = [Register::Allow, Register::Block].iter().filter_map(|g| self.regs.get(&(fp.to_string(), *g))).map(|r| r.at_ms).max();
        known.map(|k| (k + 1).max(now_ms)).unwrap_or(now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RecordKind::*;

    fn fp(n: u8) -> String {
        format!("{:02x}", n).repeat(32)
    }

    fn rec(n: u8, kind: RecordKind, at_ms: u64) -> PhoneRecord {
        PhoneRecord { fp: fp(n), name: format!("phone{n}"), kind, at_ms }
    }

    fn merged(a: &Records, b: &Records) -> Records {
        let mut m = a.clone();
        m.merge(&b.all());
        m
    }

    #[test]
    fn the_later_record_in_a_register_wins() {
        let mut r = Records::default();
        assert!(r.put(rec(1, Allow, 10)));
        assert!(r.is_allowed(&fp(1)));
        assert!(r.put(rec(1, Revoke, 20)));
        assert!(!r.is_allowed(&fp(1)));
        // An older allow arriving late changes nothing.
        assert!(!r.put(rec(1, Allow, 15)));
        assert!(!r.is_allowed(&fp(1)));
        assert!(r.put(rec(1, Allow, 30)));
        assert!(r.is_allowed(&fp(1)));
    }

    #[test]
    fn ties_fall_on_the_safe_side_in_any_order() {
        for order in [[Allow, Revoke], [Revoke, Allow]] {
            let mut r = Records::default();
            for k in order {
                r.put(rec(1, k, 50));
            }
            assert!(!r.is_allowed(&fp(1)), "{order:?}");
        }
        for order in [[Block, Unblock], [Unblock, Block]] {
            let mut r = Records::default();
            r.put(rec(1, Allow, 1));
            for k in order {
                r.put(rec(1, k, 50));
            }
            assert!(r.is_blocked(&fp(1)), "{order:?}");
        }
    }

    #[test]
    fn a_tie_between_equal_kinds_with_different_names_has_one_winner() {
        let a = PhoneRecord { name: "alpha".into(), ..rec(1, Allow, 5) };
        let b = PhoneRecord { name: "beta".into(), ..rec(1, Allow, 5) };
        let x = Records::from_list([a.clone(), b.clone()]);
        let y = Records::from_list([b, a]);
        assert_eq!(x, y);
    }

    #[test]
    fn a_block_beats_an_allow_whatever_the_order() {
        // The allow is later than the block: it still does not let the phone in.
        let block_first = Records::from_list([rec(1, Block, 10), rec(1, Allow, 20)]);
        let allow_first = Records::from_list([rec(1, Allow, 20), rec(1, Block, 10)]);
        assert_eq!(block_first, allow_first);
        assert!(block_first.is_blocked(&fp(1)));
        assert!(!block_first.is_allowed(&fp(1)));
        assert!(block_first.allowed().is_empty());
        assert_eq!(block_first.blocked().len(), 1);
    }

    #[test]
    fn a_later_unblock_lifts_the_block() {
        let mut r = Records::from_list([rec(1, Allow, 5), rec(1, Block, 10)]);
        assert!(!r.is_allowed(&fp(1)));
        r.put(rec(1, Unblock, 11));
        assert!(!r.is_blocked(&fp(1)));
        assert!(r.is_allowed(&fp(1)));
        // An unblock that is older than the block does nothing.
        let mut r = Records::from_list([rec(1, Allow, 5), rec(1, Block, 10)]);
        assert!(!r.put(rec(1, Unblock, 9)));
        assert!(r.is_blocked(&fp(1)));
        // A newer block after the unblock blocks again.
        r.put(rec(1, Unblock, 11));
        r.put(rec(1, Block, 12));
        assert!(r.is_blocked(&fp(1)));
    }

    #[test]
    fn phones_do_not_affect_each_other() {
        let r = Records::from_list([rec(1, Block, 10), rec(2, Allow, 5)]);
        assert!(r.is_blocked(&fp(1)));
        assert!(!r.is_blocked(&fp(2)));
        assert!(r.is_allowed(&fp(2)));
        assert!(!r.is_allowed(&fp(3)));
    }

    #[test]
    fn merging_is_idempotent() {
        let a = Records::from_list([rec(1, Allow, 10), rec(2, Block, 7), rec(2, Allow, 3)]);
        let b = Records::from_list([rec(1, Revoke, 12), rec(3, Allow, 1)]);
        let once = merged(&a, &b);
        assert_eq!(merged(&once, &b), once);
        assert_eq!(merged(&once, &once), once);
        assert_eq!(merged(&once, &a), once);
        let mut again = once.clone();
        assert!(again.merge(&b.all()).is_empty());
        assert!(again.merge(&a.all()).is_empty());
    }

    #[test]
    fn merging_gives_the_same_result_in_any_order() {
        let a = Records::from_list([rec(1, Allow, 10), rec(2, Block, 7), rec(4, Allow, 9)]);
        let b = Records::from_list([rec(1, Revoke, 12), rec(2, Unblock, 8), rec(3, Allow, 1)]);
        let c = Records::from_list([rec(1, Allow, 12), rec(2, Allow, 20), rec(4, Block, 9), rec(4, Unblock, 9)]);
        let ab = merged(&a, &b);
        assert_eq!(ab, merged(&b, &a));
        let abc = merged(&ab, &c);
        assert_eq!(abc, merged(&a, &merged(&b, &c)));
        assert_eq!(abc, merged(&merged(&c, &a), &b));
        assert_eq!(abc, merged(&c, &merged(&b, &a)));
        // Spot checks on the result: 1 is revoked and allowed at 12 so revoke wins, 2 was unblocked at 8 and
        // allowed at 20, 4 is blocked because the tie at 9 falls on Block.
        assert!(!abc.is_allowed(&fp(1)));
        assert!(abc.is_allowed(&fp(2)));
        assert!(abc.is_blocked(&fp(4)));
        assert!(abc.is_allowed(&fp(3)));
    }

    #[test]
    fn merge_reports_only_what_changed() {
        let mut a = Records::from_list([rec(1, Allow, 10)]);
        let changed = a.merge(&[rec(1, Allow, 10), rec(1, Allow, 9), rec(2, Block, 4)]);
        assert_eq!(changed, vec![rec(2, Block, 4)]);
    }

    #[test]
    fn records_with_a_bad_fingerprint_are_ignored() {
        let mut r = Records::default();
        assert!(!r.put(PhoneRecord { fp: "short".into(), name: "x".into(), kind: Allow, at_ms: 1 }));
        assert!(!r.put(PhoneRecord { fp: "z".repeat(64), name: "x".into(), kind: Allow, at_ms: 1 }));
        assert!(r.all().is_empty());
    }

    #[test]
    fn a_change_made_here_is_always_later_than_what_is_known() {
        let r = Records::from_list([rec(1, Block, 5_000)]);
        // This clock runs behind the one that wrote the block.
        assert_eq!(r.next_at(&fp(1), 1_000), 5_001);
        assert_eq!(r.next_at(&fp(1), 9_000), 9_000);
        assert_eq!(r.next_at(&fp(2), 1_000), 1_000);
    }
}
