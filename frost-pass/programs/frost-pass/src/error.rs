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
    #[msg("Ticket sales have ended")]
    SalesEnded,
    #[msg("Sales must end in the future and no later than the event end")]
    InvalidSalesEnd,
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
    #[msg("Ticket does not belong to this event")]
    InvalidEvent,
    #[msg("Collection must be a new account not the organizer")]
    InvalidCollection,
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
    #[msg("User does not own this ticket")]
    NotTicketOwner,
    #[msg("Signer is not the listed seller")]
    NotSeller,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Challenge expired")]
    ChallengeExpired,
    #[msg("Invalid challenge")]
    InvalidChallenge,
    #[msg("Listed tickets cannot be redeemed")]
    TicketListed,
    #[msg("Buyer cannot be the seller")]
    BuyerIsSeller,
    #[msg("Listing price exceeds max buyer price")]
    PriceExceedsMax,
}
