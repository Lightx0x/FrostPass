use anchor_lang::prelude::*;

#[constant]
pub const EVENT_SEED: &[u8] = b"event";

#[constant]
pub const TICKET_SEED: &[u8] = b"ticket";

#[constant]
pub const MAX_RESALE: u8 = 2; // Max amount of resales allowed

#[constant]
pub const MAX_ROYALTY_BPS: u16 = 500; // 5% in basis points, organizer's royalty_bps can't exceed this

#[constant]
pub const MINT_FEE: u64 = 250_000; // 0.25 USDC protocol fee per primary mint, paid by the minter on top of ticket_price

#[constant]
pub const MAX_MARKUP_BPS: u16 = 2_000; // 20% in basis points

pub const MAX_SCANNERS: usize = 5; // Max authorized scanners per event

pub const MAX_NAME_LENGTH: usize = 32; // Max string length for event name
pub const MAX_URI_LENGTH: usize = 200; // Max string length for metadata URI

pub const MIN_EVENT_DURATION: i64 = 3_600; // Minimum 1 hour (in seconds)
pub const MAX_EVENT_DURATION: i64 = 365 * 24 * 3_600; // Maximum 1 year (in seconds)

// Devnet USDC mint (used for development and testing)
#[constant]
pub const USDC_MINT: Pubkey = pubkey!("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU");

// TODO: replace with the real protocol treasury wallet before deploy
#[constant]
pub const PROTOCOL_TREASURY: Pubkey = pubkey!("FrostPassTreasuryP1aceho1der111111111111111");

pub const BPS_DENOMINATOR: u64 = 10_000; // 100% in basis points

pub const CHALLENGE_DOMAIN: &[u8; 16] = b"FROSTPASS_TKT_V1";
pub const CHALLENGE_NONCE_LENGTH: usize = 16;
pub const CHALLENGE_VALIDITY_SECONDS: i64 = 90;
