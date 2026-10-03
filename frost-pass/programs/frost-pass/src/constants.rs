use anchor_lang::prelude::*;

#[constant]
pub const RESALE_MAX: u8 = 2; // Max amount of resales allowed

#[constant]
pub const ROYALTY_BPS: u16 = 300; // 3% in basis points

#[constant]
pub const PROTOCOL_FEE_BPS: u16 = 200; // 2% in basis points

#[constant]
pub const MARKUP_MAX_BPS: u16 = 2_000; // 20% in basis points
