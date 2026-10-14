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

    /// The pass's QR modules with a 2-module quiet zone, row by row (`true` = dark), and the side
    /// length.
    fn qr_modules(&self) -> Result<(Vec<bool>, usize)> {
        let code = QrCode::with_error_correction_level(self.encode(), EcLevel::L)
            .context("the pass doesn't fit in a QR code")?;
        let width = code.width();
        let colors = code.to_colors();
        const QUIET: usize = 2;
        let size = width + 2 * QUIET;
        let modules = (0..size * size)
            .map(|i| {
                let (x, y) = (i % size, i / size);
                (QUIET..width + QUIET).contains(&x)
                    && (QUIET..width + QUIET).contains(&y)
                    && colors[(y - QUIET) * width + (x - QUIET)] == Color::Dark
            })
            .collect();
        Ok((modules, size))
    }

    /// Draws the QR with `wide` × `tall` modules per character cell, `glyph` turning a cell's
    /// modules (bit `x + wide * y`, dark = 1) into its character.
    fn qr_cells(&self, wide: usize, tall: usize, glyph: fn(u8) -> char) -> Result<Vec<String>> {
        let (modules, size) = self.qr_modules()?;
        let dark = |x: usize, y: usize| x < size && y < size && modules[y * size + x];
        Ok((0..size)
            .step_by(tall)
            .map(|y| {
                (0..size)
                    .step_by(wide)
                    .map(|x| {
                        let mut bits = 0u8;
                        for dy in 0..tall {
                            for dx in 0..wide {
                                if dark(x + dx, y + dy) {
                                    bits |= 1 << (dx + wide * dy);
                                }
                            }
                        }
                        glyph(bits)
                    })
                    .collect()
            })
            .collect())
    }

    /// The pass as a QR code, one module per column and two per row (half blocks): square
    /// modules in a normal 1:2 terminal cell. Draw it dark on light: the glyphs are the dark
    /// modules.
    pub fn qr_rows(&self) -> Result<Vec<String>> {
        self.qr_cells(1, 2, |bits| match bits {
            0b11 => '█',
            0b01 => '▀',
            0b10 => '▄',
            _ => ' ',
        })
    }

    /// The same QR at about half the size each way: 2×3 modules per cell (Unicode sextants), so
    /// it fits small terminals. Modules come out nearly square (about 3:4 in a 1:2 cell), which
    /// phone scanners read like any QR code.
    pub fn qr_rows_compact(&self) -> Result<Vec<String>> {
        self.qr_cells(2, 3, sextant)
    }
}

/// The character for a 2×3 cell whose dark sixths are `bits` (bit 0 top left, 1 top right,
/// 2 middle left … 5 bottom right). Unicode's sextant block (U+1FB00) skips the four patterns
/// that already exist elsewhere: empty, full, and the left and right halves.
fn sextant(bits: u8) -> char {
    const LEFT: u8 = 0b01_0101;
    const RIGHT: u8 = 0b10_1010;
    match bits & 0b11_1111 {
        0 => ' ',
        0b11_1111 => '█',
        LEFT => '▌',
        RIGHT => '▐',
        bits => {
            let skipped = 1 + u32::from(bits > LEFT) + u32::from(bits > RIGHT);
            char::from_u32(0x1FB00 + u32::from(bits) - skipped).unwrap_or('?')
        }
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
    fn the_compact_qr_is_the_same_code_at_half_the_size() {
        let holder = Keypair::new();
        let pass = Pass::sign(&holder, &Pubkey::new_unique(), &Pubkey::new_unique(), 0).unwrap();
        let (modules, size) = pass.qr_modules().unwrap();
        let full = pass.qr_rows().unwrap();
        let compact = pass.qr_rows_compact().unwrap();
        assert_eq!(
            (full[0].chars().count(), full.len()),
            (size, size.div_ceil(2))
        );
        assert_eq!(
            (compact[0].chars().count(), compact.len()),
            (size.div_ceil(2), size.div_ceil(3))
        );
        // Every cell of the compact code shows exactly its six modules
        let sextant_bits = |c: char| -> u8 {
            match c {
                ' ' => 0,
                '█' => 0b11_1111,
                '▌' => 0b01_0101,
                '▐' => 0b10_1010,
                c => {
                    let index = c as u32 - 0x1FB00;
                    let bits = index + 1;
                    let bits = bits + u32::from(bits >= 0b01_0101);
                    (bits + u32::from(bits >= 0b10_1010)) as u8
                }
            }
        };
        let dark = |x: usize, y: usize| x < size && y < size && modules[y * size + x];
        for (row, line) in compact.iter().enumerate() {
            for (col, c) in line.chars().enumerate() {
                let bits = sextant_bits(c);
                for i in 0..6 {
                    assert_eq!(
                        bits >> i & 1 == 1,
                        dark(col * 2 + i % 2, row * 3 + i / 2),
                        "cell {col},{row} sixth {i}"
                    );
                }
            }
        }
    }

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
