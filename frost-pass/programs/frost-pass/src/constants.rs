use anchor_lang::prelude::*;

#[constant]
pub const EVENT_SEED: &[u8] = b"event";

#[constant]
pub const TICKET_SEED: &[u8] = b"ticket";

#[constant]
pub const MAX_RESALE: u8 = 2; // Max amount of resales allowed

#[constant]
pub const MAX_ROYALTY_BPS: u16 = 300; // 3% in basis points, organizer's royalty_bps can't exceed this

#[constant]
pub const PROTOCOL_FEE_BPS: u16 = 200; // 2% in basis points

#[constant]
pub const MAX_MARKUP_BPS: u16 = 2_000; // 20% in basis points

// TODO: replace with the real protocol treasury wallet before deploy
#[constant]
pub const PROTOCOL_TREASURY: Pubkey = pubkey!("FrostPassTreasuryP1aceho1der111111111111111");

pub const BPS_DENOMINATOR: u64 = 10_000; // 100% in basis points
