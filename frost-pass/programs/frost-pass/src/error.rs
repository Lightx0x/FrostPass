use anchor_lang::prelude::*;

#[error_code]
pub enum ErrorCode {
    #[msg("Exceeds maximum allowed markup")]
    ExceedsMaxAllowedMarkup,

    #[msg("Invalid Supply amount")]
    InvalidSupplyAmount,

    #[msg("Event Sold Out")]
    EventSoldOut,

    #[msg("Exceeds maximum resale")]
    ExceedsMaximumResale,

    #[msg("Event Ended")]
    EventEnded,

    #[msg("Invalid Scanner")]
    InvalidScanner,
}
