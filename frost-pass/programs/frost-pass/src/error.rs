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
    ResaleLimitReached,
    #[msg("Event ended")]
    EventEnded,
    #[msg("Event duration too short (minimum 1 hour)")]
    EventDurationTooShort,
    #[msg("Event duration too long (maximum 1 year)")]
    EventDurationTooLong,
    #[msg("Invalid scanner")]
    InvalidScanner,
    #[msg("Too many scanners")]
    TooManyScanners,
    #[msg("No scanners provided")]
    NoScannersProvided,
    #[msg("Duplicate scanner provided")]
    DuplicateScanner,
    #[msg("Exceeds maximum allowed royalty")]
    ExceedsMaxAllowedRoyalty,
    #[msg("Invalid event initialization")]
    InvalidEvent,
    #[msg("Invalid ticket price")]
    InvalidPrice,
    #[msg("Invalid event name length")]
    InvalidNameLength,
    #[msg("Invalid metadata URI length")]
    InvalidUriLength,
    #[msg("Invalid USDC mint")]
    InvalidUsdcMint,
    #[msg("Resale price above markup cap")]
    PriceAboveCap,
    #[msg("Ticket not listed")]
    NotListed,
    #[msg("Ticket already listed")]
    AlreadyListed,
    #[msg("Signer does not hold this ticket")]
    NotTicketOwner,
    #[msg("Signer is not the listed seller")]
    NotSeller,
    #[msg("Token account has the wrong owner")]
    InvalidOwner,
    #[msg("Math overflow")]
    MathOverflow,
}
