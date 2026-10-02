use anchor_lang::prelude::*;

#[error_code]
pub enum ErrorCode {
    #[msg("Exceeds maximum allowed markup")]
    ExceedsMaxAllowedMarkup,

    #[msg("Invalid supply amount")]
    InvalidSupplyAmount,

    #[msg("Event sold out")]
    EventSoldOut,

    #[msg("Exceeds maximum resale")]
    ExceedsMaximumResale,

    #[msg("Event ended")]
    EventEnded,

    #[msg("Invalid scanner")]
    InvalidScanner,

    #[msg("Exceeds maximum allowed royalty")]
    ExceedsMaxAllowedRoyalty,

    #[msg("Invalid event initialization")]
    InvalidEvent,

    #[msg("No ticket price set")]
    InvalidPrice,

    #[msg("Protocol fees not included")]
    InvalidFee,
}
