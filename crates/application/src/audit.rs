//! # The audit chain
//!
//! Computing and checking the hashes that make the audit trail tamper-evident.
//!
//! ## Why this is not in `domain`
//!
//! `domain` decides what a legal transaction is. It has no opinion about how the
//! evidence that one happened is stored, and it must not, because a hash function
//! is a storage concern: change it and every historical entry becomes unreadable.
//! [`crate::ports::AuditLog`] is the contract, this module is the arithmetic both
//! implementations share, and the adapter is only responsible for the locking.
//!
//! ## Why both implementations share it
//!
//! The alternative — letting the Postgres adapter hash rows and letting the
//! in-memory fake hash them its own way — is the kind of duplication that looks
//! harmless and is not. The fake and the adapter would agree until the first
//! difference in encoding, at which point [`crate::usecases::reconciliation`]
//! would report [`AuditChainBroken`](crate::usecases::reconciliation::Violation::AuditChainBroken)
//! on a perfectly intact database, and the operator chasing it would be sent to
//! look for tampering rather than for a separator character. One function, called
//! by both, makes that class of bug impossible rather than unlikely.
//!
//! ## What the hash covers
//!
//! Every stored field, including [`AuditRecord::occurred_at_unix`]. The timestamp
//! is the interesting one: it is the field most worth falsifying, because
//! "when did this happen" is the first question asked after an incident, and an
//! entry whose time has been moved to fall inside a shift that somebody else was
//! on duty is worth a great deal to whoever is caught. An earlier version of this
//! chain did not cover it, which meant an auditor could rewrite history's
//! timeline without breaking a single link. It is covered now, and the coverage
//! costs nothing because the timestamp is written by the same code that hashes.
//!
//! ## Why the fields are length-prefixed
//!
//! Naive concatenation is not injective. Hashing `"ab" + "c"` produces the same
//! digest as hashing `"a" + "bc"`, so a `subject` of `ab` and an `actor` of `c`
//! are indistinguishable from a `subject` of `a` and an `actor` of `bc` — an
//! attacker can move a fact between fields and leave the chain intact. Every field
//! is therefore written as its length followed by its bytes, which makes the
//! encoding injective by construction rather than by hoping no field contains the
//! separator. (The separator approach this replaced was only safe while every
//! field was known not to contain it, and `detail` is caller-supplied JSON.)
//!
//! ## What this does and does not defend against
//!
//! It proves the chain has not been edited *in place*. It does not prove an
//! attacker with write access did not also delete a suffix: truncating the tail
//! leaves the surviving links all valid. Catching that needs the head hash
//! recorded somewhere the same writer cannot reach — an external notary, a
//! published digest, a second system. [`verify_chain`] is honest about this and
//! takes the expected head so a caller that has one can use it; see its docs.

use sha2::{Digest, Sha256};

use crate::ports::{AuditRecord, NewAuditEntry};

/// What the first entry chains to.
///
/// A fixed constant rather than an empty string, so that an entry with no
/// predecessor can never collide with an entry whose predecessor hashed to
/// nothing, and so the first hash is reproducible from the schema alone.
pub const GENESIS_HASH: &str = "ladywalk.audit.genesis.v1";

/// Separates this chain's hashes from every other use of SHA-256 in the system.
///
/// A domain-separation tag, not decoration. Quote signing also uses SHA-256, and
/// a signed quote and an audit entry with identical bytes must not produce
/// identical digests — otherwise a signature computed for one is a valid
/// substitution for the other, and the two have completely different meanings.
const DOMAIN: &[u8] = b"ladywalk.audit.chain.v1";

/// Hash one entry, chaining it to `previous_hash`.
///
/// Covers every stored field of the record, so that changing any of them — the
/// action, the subject, who did it, the detail, when, the position, or the entry
/// before it — produces a different hash and therefore a detectable break.
///
/// `previous_hash` is the value stored in [`AuditRecord::previous_hash`], not a
/// re-read of the current head, so that verification and append compute over
/// exactly the same bytes.
#[must_use]
pub fn entry_hash(
    seq: i64,
    previous_hash: &str,
    occurred_at_unix: i64,
    entry: &NewAuditEntry,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    field(&mut hasher, &seq.to_string());
    field(&mut hasher, previous_hash);
    field(&mut hasher, &occurred_at_unix.to_string());
    field(&mut hasher, &entry.action);
    field(&mut hasher, &entry.subject);
    field(&mut hasher, &entry.actor);
    field(&mut hasher, &entry.detail);
    hex::encode(hasher.finalize())
}

/// Feed one length-prefixed field into the hash.
///
/// The length is written as a decimal count followed by a colon, which is
/// unambiguous because the count is terminated by a character that cannot appear
/// in a decimal number, so the reader always knows where the count ends and
/// therefore where the field begins.
fn field(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_string().as_bytes());
    hasher.update(b":");
    hasher.update(value.as_bytes());
}

/// What is wrong with a chain, if anything.
///
/// Separate from a bare `bool` because "the chain is broken" and "the chain ends
/// at 41 when it should end at 900" are very different pages for whoever is on
/// call, and a boolean forces them to work that out by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainBreak {
    /// The entry at this position claims a position it does not have.
    ///
    /// A gap, which usually means a rolled-back insert rather than tampering —
    /// the one case that is benign more often than not.
    Discontinuity {
        /// The position the entry claims.
        expected_seq: i64,
        /// The position it actually occupies.
        found_seq: i64,
    },
    /// The entry does not chain to the one before it.
    ///
    /// The serious one. Either a row was edited, or two writers appended without
    /// serialising on the head and one of them overwrote the other's link.
    BrokenLink {
        /// The position of the entry whose link is wrong.
        seq: i64,
    },
    /// The entry's contents do not match its own hash.
    ///
    /// Also serious, and also ambiguous between "someone edited a row" and "the
    /// implementation hashed something other than what it stored".
    Rewritten {
        /// The position of the entry that does not match.
        seq: i64,
    },
    /// The chain ends before the head the caller expected.
    ///
    /// Only reported when a head is supplied, because without one there is no way
    /// to tell a truncated chain from a complete short one.
    Truncated {
        /// The last position the caller expected to find.
        expected_seq: i64,
    },
}

impl ChainBreak {
    /// A short, stable name for a metric label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Discontinuity { .. } => "audit_chain.discontinuity",
            Self::BrokenLink { .. } => "audit_chain.broken_link",
            Self::Rewritten { .. } => "audit_chain.rewritten",
            Self::Truncated { .. } => "audit_chain.truncated",
        }
    }
}

/// Check a chain, returning the first thing wrong with it.
///
/// Pass `expected_head_seq` when the caller knows what the last position should
/// be — a stored head, a count from a previous run — and it will also catch a
/// chain that has been cut short. Omit it (`None`) and a truncated tail is
/// undetectable, which is the limit of what a single table can prove about
/// itself; see the module documentation.
///
/// Verification recomputes each entry's hash from the fields actually stored, so
/// it detects an edit to any of them, and checks each `previous_hash` against the
/// entry before it, so it detects a row being inserted, removed, or reordered.
/// It deliberately does not re-derive `occurred_at_unix` from anything: the
/// timestamp is a stored fact with no other source, so covering it in the hash is
/// what makes it trustworthy, not comparing it.
#[must_use]
pub fn verify_chain(chain: &[AuditRecord], expected_head_seq: Option<i64>) -> Option<ChainBreak> {
    let mut previous_hash = GENESIS_HASH.to_string();

    for (index, record) in chain.iter().enumerate() {
        let expected_seq = i64::try_from(index + 1).ok()?;
        if record.seq != expected_seq {
            return Some(ChainBreak::Discontinuity {
                expected_seq,
                found_seq: record.seq,
            });
        }
        if record.previous_hash != previous_hash {
            return Some(ChainBreak::BrokenLink { seq: record.seq });
        }
        // Reconstruct the entry exactly as it was appended. Only these four
        // fields plus the timestamp are hashed, so this is the whole of what the
        // hash commits to; if that ever stops being true, this function and
        // `entry_hash` have to change together, and the version in `DOMAIN` has to
        // change with them or every existing chain becomes unreadable.
        let recomputed = entry_hash(
            record.seq,
            &record.previous_hash,
            record.occurred_at_unix,
            &NewAuditEntry {
                action: record.action.clone(),
                subject: record.subject.clone(),
                actor: record.actor.clone(),
                detail: record.detail.clone(),
            },
        );
        if recomputed != record.hash {
            return Some(ChainBreak::Rewritten { seq: record.seq });
        }
        previous_hash = record.hash.clone();
    }

    if let Some(expected) = expected_head_seq {
        let actual = chain.last().map_or(0, |record| record.seq);
        if actual != expected {
            return Some(ChainBreak::Truncated {
                expected_seq: expected,
            });
        }
    }

    None
}
