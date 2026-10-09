//! # Quote signing
//!
//! The HMAC-SHA256 signer behind [`QuoteSigner`], and the system clock behind
//! [`Clock`].
//!
//! ## Why the secret lives here and not in `domain`
//!
//! `domain` decides what a quote says. What makes it *true* is a secret, and a
//! secret is infrastructure: it comes from configuration, it must never be
//! logged, and it must never be reachable from a crate whose whole reason to
//! exist is being readable end to end. So the domain builds an unsigned quote,
//! this module signs the bytes, and nothing in between can read the key.
//!
//! ## The key is compared in constant time
//!
//! [`HmacSigner::verify`] delegates to the constant-time comparison in
//! `domain::quote::constant_time_eq` rather than `==`. A byte-by-byte comparison
//! that returns early leaks how much of a forged signature was correct, which
//! is enough to recover a real signature one byte at a time. The `subtle`
//! dependency is deliberately not taken: the domain already needs the
//! comparison, and the money path's dependency count is a promise the README
//! makes.

use std::fmt;

use async_trait::async_trait;
use domain::error::DomainError;
use domain::quote::constant_time_eq;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::ports::{Clock, QuoteSigner};

/// Signs quotes with HMAC-SHA256.
#[derive(Clone)]
pub struct HmacSigner {
    key: Vec<u8>,
}

impl HmacSigner {
    /// Build a signer from a secret.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Internal`] if the key is shorter than 32 bytes.
    /// HMAC accepts any key length, but a short one is a configuration mistake
    /// and one that is far easier to make by accident with a passphrase than
    /// with a generated secret, so it is refused here rather than discovered in
    /// a security review.
    pub fn new(key: &[u8]) -> Result<Self, DomainError> {
        const MIN_KEY_LEN: usize = 32;
        if key.len() < MIN_KEY_LEN {
            return Err(DomainError::internal(format!(
                "the quote signing key must be at least {MIN_KEY_LEN} bytes, got {}",
                key.len()
            )));
        }
        Ok(Self {
            key: key.to_vec(),
        })
    }

    /// The length of the key, for a log line that must not leak the key.
    #[must_use]
    pub fn key_len(&self) -> usize {
        self.key.len()
    }
}

impl fmt::Debug for HmacSigner {
    /// Prints the key length and nothing else.
    ///
    /// A derived `Debug` would print the key. `#[derive(Debug)]` on anything
    /// holding a secret is a leak waiting for someone to log the struct.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HmacSigner")
            .field("key_len", &self.key.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl QuoteSigner for HmacSigner {
    async fn sign(&self, signing_input: &str) -> Result<String, DomainError> {
        // `new_from_slice` only fails for an empty key, which the constructor
        // already excluded, so an error here would be unreachable. It is
        // mapped to an internal error rather than unwrapped: the no-panic rule
        // exists for code that runs in production.
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key)
            .map_err(|e| DomainError::internal(format!("could not initialise the MAC: {e}")))?;
        mac.update(signing_input.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    async fn verify(&self, signing_input: &str, signature: &str) -> Result<bool, DomainError> {
        let expected = self.sign(signing_input).await?;
        Ok(constant_time_eq(expected.as_bytes(), signature.as_bytes()))
    }
}

/// The system clock, in seconds since the Unix epoch.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        i64::try_from(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()))
            .unwrap_or(i64::MAX)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"a-32-byte-test-key-for-signing!!";

    /// A second key of the same length, for proving a signature is bound to the
    /// key that made it.
    const OTHER_KEY: &[u8] = b"another-32-byte-test-key-for-signing!";

    #[test]
    fn the_key_must_be_long_enough() {
        assert!(HmacSigner::new(b"too short").is_err());
        assert!(HmacSigner::new(&[0_u8; 31]).is_err());
        assert!(HmacSigner::new(KEY).is_ok());
    }

    #[test]
    fn a_signature_is_stable_for_the_same_input() {
        let signer = HmacSigner::new(KEY).expect("valid key");
        let block = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        block.block_on(async {
            let first = signer.sign("m1\x1fhome\x1f25000").await.expect("signs");
            let second = signer.sign("m1\x1fhome\x1f25000").await.expect("signs");
            assert_eq!(first, second);
            assert_eq!(first.len(), 64, "expected hex-encoded sha256");
        });
    }

    #[test]
    fn a_different_input_gives_a_different_signature() {
        let signer = HmacSigner::new(KEY).expect("valid key");
        let block = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        block.block_on(async {
            let a = signer.sign("m1\x1fhome").await.expect("signs");
            let b = signer.sign("m1\x1faway").await.expect("signs");
            assert_ne!(a, b);
        });
    }

    #[test]
    fn verification_accepts_its_own_signature_and_rejects_a_forged_one() {
        let signer = HmacSigner::new(KEY).expect("valid key");
        let other = HmacSigner::new(OTHER_KEY).expect("valid key");
        let block = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        block.block_on(async {
            let signature = signer.sign("payload").await.expect("signs");
            assert!(signer.verify("payload", &signature).await.expect("verifies"));
            assert!(!signer.verify("payload!", &signature).await.expect("verifies"));
            assert!(
                !other.verify("payload", &signature).await.expect("verifies"),
                "a different key must not validate a signature"
            );
        });
    }

    #[test]
    fn debug_output_does_not_contain_the_key() {
        let signer = HmacSigner::new(KEY).expect("valid key");
        let rendered = format!("{signer:?}");
        let key = String::from_utf8_lossy(KEY).into_owned();
        assert!(!rendered.contains(&key));
        assert!(rendered.contains("key_len"));
    }
}
