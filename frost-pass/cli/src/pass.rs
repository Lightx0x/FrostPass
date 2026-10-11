//! The door pass: the ticket holder's signature over a fresh redeem challenge, shown as a QR code
//! and scanned (or pasted) at the door.
//!
//! A pass is `FROSTPASS:1:` followed by unpadded base32 (RFC 4648) of
//! `ticket_asset (32) || user (32) || nonce (16) || expiry i64 LE (8) || signature (64)`.
//! It's all upper-case letters, digits and `:`, so the QR code uses its compact alphanumeric
//! mode and fits a normal terminal.
//!
//! The event isn't in the pass: the scanner reads it from the ticket's `TicketState`. The holder
//! still signs the full 136-byte challenge, event included, so a pass can't be replayed at
//! another event.

use anchor_lang::prelude::Pubkey;
use anyhow::{bail, ensure, Context, Result};
use frost_pass::constants::CHALLENGE_NONCE_LENGTH;
use frost_pass_client::RedeemChallenge;
use qrcode::{Color, EcLevel, QrCode};
use solana_keypair::Keypair;
use solana_signature::Signature;
use solana_signer::Signer;

const PREFIX: &str = "FROSTPASS:1:";
const PAYLOAD_LENGTH: usize = 32 + 32 + CHALLENGE_NONCE_LENGTH + 8 + 64;
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// How long a new pass is valid. The program accepts at most `CHALLENGE_VALIDITY_SECONDS` (90s);
/// 60s leaves the scanner time to submit.
pub const PASS_LIFETIME_SECONDS: i64 = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pass {
    pub ticket_asset: Pubkey,
    pub user: Pubkey,
    pub nonce: [u8; CHALLENGE_NONCE_LENGTH],
    pub expiry: i64,
    pub signature: [u8; 64],
}

impl Pass {
    /// Signs a new challenge for a ticket of `event_config` with the holder's wallet, valid
    /// until `now + PASS_LIFETIME_SECONDS` (cluster time).
    pub fn sign(
        holder: &Keypair,
        event_config: &Pubkey,
        ticket_asset: &Pubkey,
        now: i64,
    ) -> Result<Self> {
        let mut nonce = [0u8; CHALLENGE_NONCE_LENGTH];
        getrandom::getrandom(&mut nonce).context("no randomness for the pass nonce")?;
        let challenge = RedeemChallenge {
            event_config: *event_config,
            ticket_asset: *ticket_asset,
            user: holder.pubkey(),
            nonce,
            expiry: now + PASS_LIFETIME_SECONDS,
        };
        Ok(Self {
            ticket_asset: *ticket_asset,
            user: holder.pubkey(),
            nonce,
            expiry: challenge.expiry,
            signature: holder.sign_message(&challenge.message()).into(),
        })
    }

    /// The challenge this pass signs, for the ticket's event.
    pub fn challenge(&self, event_config: &Pubkey) -> RedeemChallenge {
        RedeemChallenge {
            event_config: *event_config,
            ticket_asset: self.ticket_asset,
            user: self.user,
            nonce: self.nonce,
            expiry: self.expiry,
        }
    }

    /// Whether `user` signed this pass for a ticket of `event_config`.
    pub fn verify(&self, event_config: &Pubkey) -> bool {
        Signature::from(self.signature)
            .verify(self.user.as_ref(), &self.challenge(event_config).message())
    }

    pub fn encode(&self) -> String {
        let mut bytes = Vec::with_capacity(PAYLOAD_LENGTH);
        bytes.extend_from_slice(self.ticket_asset.as_ref());
        bytes.extend_from_slice(self.user.as_ref());
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.expiry.to_le_bytes());
        bytes.extend_from_slice(&self.signature);
        format!("{PREFIX}{}", base32_encode(&bytes))
    }

    /// Reads a pass, ignoring case and any whitespace (e.g. line breaks from copying it).
    pub fn decode(text: &str) -> Result<Self> {
        let text: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        let Some(payload) = text.strip_prefix(PREFIX) else {
            bail!("not a FrostPass pass (it should start with {PREFIX})");
        };
        let bytes = base32_decode(payload).context("the pass is damaged")?;
        ensure!(
            bytes.len() == PAYLOAD_LENGTH,
            "the pass is damaged ({} bytes instead of {PAYLOAD_LENGTH})",
            bytes.len()
        );
        let key = |at: usize| Pubkey::new_from_array(bytes[at..at + 32].try_into().unwrap());
        let (nonce_at, expiry_at, signature_at) =
            (64, 64 + CHALLENGE_NONCE_LENGTH, 72 + CHALLENGE_NONCE_LENGTH);
        Ok(Self {
            ticket_asset: key(0),
            user: key(32),
            nonce: bytes[nonce_at..expiry_at].try_into()?,
            expiry: i64::from_le_bytes(bytes[expiry_at..signature_at].try_into()?),
            signature: bytes[signature_at..].try_into()?,
        })
    }

    /// The pass as a QR code, two modules per text row (half blocks) with a quiet zone. Draw it
    /// dark on light: the block characters are the dark modules.
    pub fn qr_rows(&self) -> Result<Vec<String>> {
        let code = QrCode::with_error_correction_level(self.encode(), EcLevel::L)
            .context("the pass doesn't fit in a QR code")?;
        let width = code.width();
        let colors = code.to_colors();
        const QUIET: usize = 2;
        let size = width + 2 * QUIET;
        let dark = |x: usize, y: usize| {
            (QUIET..width + QUIET).contains(&x)
                && (QUIET..width + QUIET).contains(&y)
                && colors[(y - QUIET) * width + (x - QUIET)] == Color::Dark
        };
        Ok((0..size)
            .step_by(2)
            .map(|y| {
                (0..size)
                    .map(|x| match (dark(x, y), dark(x, y + 1)) {
                        (true, true) => '█',
                        (true, false) => '▀',
                        (false, true) => '▄',
                        (false, false) => ' ',
                    })
                    .collect()
            })
            .collect())
    }
}

fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_decode(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let value = BASE32
            .iter()
            .position(|&b| b == c)
            .with_context(|| format!("'{}' isn't a pass character", c as char))?;
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_round_trips_and_detects_tampering() {
        let holder = Keypair::new();
        let (event, ticket) = (Pubkey::new_unique(), Pubkey::new_unique());
        let pass = Pass::sign(&holder, &event, &ticket, 1_000).unwrap();
        assert!(pass.verify(&event));
        assert_eq!(pass.expiry, 1_000 + PASS_LIFETIME_SECONDS);

        // Survives copying: case and line breaks don't matter
        let text = pass.encode();
        assert!(text
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b':'));
        let wrapped = format!("{}\n  {}", &text[..40], text[40..].to_lowercase());
        assert_eq!(Pass::decode(&wrapped).unwrap(), pass);

        // Changing a signed field, the claimed holder, or the event breaks the signature
        let extended = Pass {
            expiry: pass.expiry + 60,
            ..pass.clone()
        };
        assert!(!extended.verify(&event));
        let stolen = Pass {
            user: Keypair::new().pubkey(),
            ..pass.clone()
        };
        assert!(!stolen.verify(&event));
        assert!(
            !pass.verify(&Pubkey::new_unique()),
            "a pass is bound to its event"
        );

        assert!(Pass::decode("FROSTPASS:1:ABC").is_err());
        assert!(Pass::decode("FROSTPASS:1:ABC0").is_err(), "0 isn't base32");
        assert!(Pass::decode("hello").is_err());

        // Version 8 (49 modules): 27 text rows with the quiet zone
        let rows = pass.qr_rows().unwrap();
        assert_eq!(rows.len(), 27);
        assert_eq!(rows[0].chars().count(), 53);
    }
}
