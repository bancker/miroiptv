//! The adult-content PIN: 4 digits, stored only as a salted SHA-256 hash, and
//! a gate that blocks PIN input for 30 s after 3 wrong attempts in a row.

use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinHash {
    pub salt: String,
    pub hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinError {
    NotFourDigits,
    NoRandom,
}

pub fn valid_pin(pin: &str) -> bool {
    pin.len() == 4 && pin.bytes().all(|b| b.is_ascii_digit())
}

impl PinHash {
    /// A hash of `pin` with a fresh random salt.
    pub fn new(pin: &str) -> Result<Self, PinError> {
        let mut salt = [0u8; 16];
        SystemRandom::new()
            .fill(&mut salt)
            .map_err(|_| PinError::NoRandom)?;
        Self::with_salt(pin, salt)
    }

    pub fn with_salt(pin: &str, salt: [u8; 16]) -> Result<Self, PinError> {
        if !valid_pin(pin) {
            return Err(PinError::NotFourDigits);
        }
        Ok(Self {
            salt: hex(&salt),
            hash: hex(&sha256(&salt, pin)),
        })
    }

    pub fn verify(&self, pin: &str) -> bool {
        let Some(salt) = unhex(&self.salt) else {
            return false;
        };
        valid_pin(pin) && hex(&sha256(&salt, pin)) == self.hash
    }
}

#[derive(Debug, Default)]
pub struct PinGate {
    failures: u32,
    blocked_until: Option<Instant>,
}

impl PinGate {
    pub const MAX_FAILURES: u32 = 3;
    pub const BLOCK: Duration = Duration::from_secs(30);

    pub fn new() -> Self {
        Self::default()
    }

    /// Time left while PIN input is blocked.
    pub fn wait_left(&self, now: Instant) -> Option<Duration> {
        self.blocked_until
            .and_then(|t| t.checked_duration_since(now))
            .filter(|d| !d.is_zero())
    }

    /// Count an attempt: right resets, the third wrong one in a row blocks.
    pub fn record(&mut self, ok: bool, now: Instant) {
        if ok {
            self.failures = 0;
            self.blocked_until = None;
            return;
        }
        self.failures += 1;
        if self.failures >= Self::MAX_FAILURES {
            self.failures = 0;
            self.blocked_until = Some(now + Self::BLOCK);
        }
    }
}

/// The forgotten-PIN reset: whoever knows the portal password owns the app.
pub fn portal_password_ok(input: &str, actual: &str) -> bool {
    !actual.is_empty() && input == actual
}

fn sha256(salt: &[u8], pin: &str) -> Vec<u8> {
    let mut ctx = digest::Context::new(&digest::SHA256);
    ctx.update(salt);
    ctx.update(pin.as_bytes());
    ctx.finish().as_ref().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_four_ascii_digits_make_a_pin() {
        assert!(valid_pin("0042"));
        for bad in ["", "123", "12345", "12a4", " 123", "١٢٣٤"] {
            assert!(!valid_pin(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_hash_verifies_the_right_pin_only() {
        let h = PinHash::with_salt("1234", [7; 16]).unwrap();
        assert!(h.verify("1234"));
        assert!(!h.verify("1235"));
        assert!(!h.verify("12345"));
        assert_eq!(h.salt, "07070707070707070707070707070707");
        assert_eq!(
            h.hash,
            "e1a48ec3f020a4ce7f6374c7e656ef713d164ebea476defc93842f97b85194c4"
        );
    }

    #[test]
    fn a_known_salt_gives_the_known_sha256() {
        // SHA-256 over 16 zero bytes followed by "0000".
        let h = PinHash::with_salt("0000", [0; 16]).unwrap();
        assert_eq!(
            h.hash,
            "06c5175f3f41bb1c8d5063c19366a643bae38a113e20b39d96405ebe8fd521e1"
        );
    }

    #[test]
    fn only_a_valid_pin_gets_a_hash() {
        assert_eq!(
            PinHash::with_salt("12", [0; 16]),
            Err(PinError::NotFourDigits)
        );
    }

    #[test]
    fn every_new_pin_gets_its_own_salt() {
        let (a, b) = (PinHash::new("1234").unwrap(), PinHash::new("1234").unwrap());
        assert_ne!(a.salt, b.salt);
        assert!(a.verify("1234") && b.verify("1234"));
    }

    #[test]
    fn a_damaged_hash_never_verifies() {
        let mut h = PinHash::with_salt("1234", [1; 16]).unwrap();
        h.salt = "zz".into();
        assert!(!h.verify("1234"));
    }

    #[test]
    fn three_wrong_attempts_in_a_row_block_for_thirty_seconds() {
        let t0 = Instant::now();
        let mut g = PinGate::new();
        g.record(false, t0);
        g.record(false, t0);
        assert_eq!(g.wait_left(t0), None);
        g.record(false, t0);
        assert_eq!(g.wait_left(t0), Some(Duration::from_secs(30)));
        assert_eq!(
            g.wait_left(t0 + Duration::from_secs(29)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(g.wait_left(t0 + Duration::from_secs(30)), None);
    }

    #[test]
    fn a_right_pin_resets_the_count() {
        let t0 = Instant::now();
        let mut g = PinGate::new();
        g.record(false, t0);
        g.record(false, t0);
        g.record(true, t0);
        g.record(false, t0);
        assert_eq!(g.wait_left(t0), None);
    }

    #[test]
    fn the_portal_password_resets_the_pin() {
        assert!(portal_password_ok("geheim", "geheim"));
        assert!(!portal_password_ok("Geheim", "geheim"));
        assert!(!portal_password_ok("", ""), "no password known: no reset");
    }
}
