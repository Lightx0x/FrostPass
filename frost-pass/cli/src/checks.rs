//! The program's rules, checked before a transaction is built.
//!
//! Each function mirrors the `require!`s of one instruction, in the same order and under the
//! same error names, so a blocked action names the rule the program would have failed with. The
//! program stays the authority: these checks only stop the app from sending transactions that
//! are certain to fail, and from offering actions that break an invariant.
//!
//! A few rules are the app's own (`rule` names that aren't program errors): enough USDC, and the
//! token accounts a payment needs.

use anchor_lang::prelude::Pubkey;
use frost_pass::{
    constants::{
        BPS_DENOMINATOR, CHALLENGE_VALIDITY_SECONDS, MAX_EVENT_DURATION, MAX_MARKUP_BPS,
        MAX_MINTS_PER_WALLET, MAX_NAME_LENGTH, MAX_RESALE, MAX_ROYALTY_BPS, MAX_SCANNERS,
        MAX_URI_LENGTH, MIN_EVENT_DURATION,
    },
    state::{EventConfig, TicketState},
};

use crate::format;

/// A rule an action would break. `rule` is the program's error name when the program enforces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub rule: &'static str,
    pub message: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}]", self.message, self.rule)
    }
}

impl std::error::Error for Violation {}

pub type Check = Result<(), Violation>;

fn rule(holds: bool, rule: &'static str, message: impl Into<String>) -> Check {
    if holds {
        Ok(())
    } else {
        Err(Violation {
            rule,
            message: message.into(),
        })
    }
}

/// The arguments of `init_event`.
#[derive(Clone, Debug)]
pub struct NewEvent {
    pub event_id: u32,
    pub name: String,
    pub uri: String,
    pub ticket_price: u64,
    pub ticket_supply: u32,
    pub markup_cap_bps: u16,
    pub royalty_bps: u16,
    pub sales_end: i64,
    pub event_end: i64,
    pub scanners: Vec<Pubkey>,
}

/// `init_event`.
pub fn new_event(event: &NewEvent, organizer: &Pubkey, collection: &Pubkey, now: i64) -> Check {
    rule(
        event.event_end >= now.saturating_add(MIN_EVENT_DURATION),
        "EventDurationTooShort",
        "The event must end at least 1 hour from now",
    )?;
    rule(
        event.event_end <= now.saturating_add(MAX_EVENT_DURATION),
        "EventDurationTooLong",
        "The event must end within 1 year",
    )?;
    rule(
        event.sales_end > now && event.sales_end <= event.event_end,
        "InvalidSalesEnd",
        "Sales must end in the future and no later than the event",
    )?;
    rule(
        event.ticket_supply > 0,
        "InvalidSupplyAmount",
        "Supply must be at least 1 ticket",
    )?;
    rule(
        event.ticket_price > 0,
        "InvalidPrice",
        "The ticket price must be above 0",
    )?;
    rule(
        event.markup_cap_bps <= MAX_MARKUP_BPS,
        "ExceedsMaxAllowedMarkup",
        format!(
            "The resale markup cap can be at most {}",
            format::percent(MAX_MARKUP_BPS)
        ),
    )?;
    rule(
        event.royalty_bps <= MAX_ROYALTY_BPS,
        "ExceedsMaxAllowedRoyalty",
        format!(
            "The royalty can be at most {}",
            format::percent(MAX_ROYALTY_BPS)
        ),
    )?;
    rule(
        !event.name.is_empty() && event.name.len() <= MAX_NAME_LENGTH,
        "InvalidNameLength",
        format!("The name must be 1–{MAX_NAME_LENGTH} bytes"),
    )?;
    rule(
        !event.uri.is_empty() && event.uri.len() <= MAX_URI_LENGTH,
        "InvalidUriLength",
        format!("The metadata URI must be 1–{MAX_URI_LENGTH} bytes"),
    )?;
    rule(
        collection != organizer,
        "InvalidCollection",
        "The collection must be a new account",
    )?;
    scanners(&event.scanners)
}

/// `validate_scanners`, shared by `init_event` and `update_scanners`.
pub fn scanners(scanners: &[Pubkey]) -> Check {
    rule(
        !scanners.is_empty(),
        "NoScannersProvided",
        "Add at least one scanner",
    )?;
    rule(
        scanners.len() <= MAX_SCANNERS,
        "TooManyScanners",
        format!("At most {MAX_SCANNERS} scanners"),
    )?;
    rule(
        !scanners.contains(&Pubkey::default()),
        "InvalidScanner",
        "The default address can't be a scanner",
    )?;
    for (index, scanner) in scanners.iter().enumerate() {
        rule(
            !scanners[index + 1..].contains(scanner),
            "DuplicateScanner",
            format!("{} is listed twice", format::short(scanner)),
        )?;
    }
    Ok(())
}

/// Organizer-only instructions. On-chain a non-organizer fails the `EventConfig` seeds check
/// (`ConstraintSeeds`) before `has_one`, but the rule is the same.
fn organizer(event: &EventConfig, signer: &Pubkey) -> Check {
    rule(
        event.organizer == *signer,
        "NotOrganizer",
        "Only the event's organizer can do this",
    )
}

/// `update_scanners`.
pub fn update_scanners(event: &EventConfig, signer: &Pubkey, list: &[Pubkey]) -> Check {
    organizer(event, signer)?;
    scanners(list)
}

/// `cancel_event`.
pub fn cancel(event: &EventConfig, signer: &Pubkey) -> Check {
    organizer(event, signer)?;
    rule(
        !event.cancelled,
        "EventCancelled",
        "The event is already cancelled",
    )
}

/// The payer and the payee of a USDC transfer must be different wallets: Anchor rejects the
/// same account passed twice as writable (`ConstraintDuplicateMutableAccount`).
fn not_organizer(event: &EventConfig, wallet: &Pubkey, message: &str) -> Check {
    rule(
        event.organizer != *wallet,
        "ConstraintDuplicateMutableAccount",
        message,
    )
}

/// `mint_ticket`. `minted_by_wallet` is the minter's `MinterRecord.count`.
pub fn mint(event: &EventConfig, minter: &Pubkey, minted_by_wallet: u8, now: i64) -> Check {
    mint_many(event, minter, minted_by_wallet, 1, now)
}

/// `count` mints in one transaction: the program checks each `mint_ticket` in turn, so the whole
/// batch must fit under the per-wallet limit and the remaining supply.
pub fn mint_many(
    event: &EventConfig,
    minter: &Pubkey,
    minted_by_wallet: u8,
    count: u8,
    now: i64,
) -> Check {
    not_organizer(
        event,
        minter,
        "Organizers can't mint their own event's tickets (they'd pay themselves)",
    )?;
    rule(!event.cancelled, "EventCancelled", "The event is cancelled")?;
    rule(
        now < event.sales_end,
        "SalesEnded",
        "Ticket sales have ended",
    )?;
    rule(
        minted_by_wallet < MAX_MINTS_PER_WALLET,
        "MintLimitReached",
        format!("Each wallet can mint at most {MAX_MINTS_PER_WALLET} tickets of an event"),
    )?;
    rule(
        event.tickets_minted < event.ticket_supply,
        "EventSoldOut",
        "The event is sold out",
    )?;
    let wallet_left = MAX_MINTS_PER_WALLET - minted_by_wallet;
    rule(
        count <= wallet_left,
        "MintLimitReached",
        format!(
            "You can mint {wallet_left} more (at most {MAX_MINTS_PER_WALLET} per wallet per event)"
        ),
    )?;
    let supply_left = event.ticket_supply - event.tickets_minted;
    rule(
        u32::from(count) <= supply_left,
        "EventSoldOut",
        format!("Only {supply_left} tickets are left"),
    )
}

/// The highest allowed resale price: `ticket_price * (100% + markup cap)`, from the original
/// price (never the last sale), in `u128` like the program.
pub fn resale_cap(event: &EventConfig) -> u128 {
    event.ticket_price as u128 * (BPS_DENOMINATOR as u128 + event.markup_cap_bps as u128)
        / BPS_DENOMINATOR as u128
}

/// `list_ticket`. `asset_owner` is the ticket's owner on the Core asset.
pub fn list(
    event: &EventConfig,
    ticket: &TicketState,
    asset_owner: &Pubkey,
    seller: &Pubkey,
    price: u64,
    now: i64,
) -> Check {
    rule(!event.cancelled, "EventCancelled", "The event is cancelled")?;
    rule(
        now < event.sales_end,
        "SalesEnded",
        "Ticket sales have ended",
    )?;
    rule(
        asset_owner == seller,
        "NotTicketOwner",
        "You don't own this ticket",
    )?;
    rule(
        ticket.seller.is_none(),
        "AlreadyListed",
        "The ticket is already listed",
    )?;
    rule(
        ticket.resale_count < MAX_RESALE,
        "ResaleLimitReached",
        format!("A ticket can be resold at most {MAX_RESALE} times"),
    )?;
    let cap = resale_cap(event);
    rule(
        price as u128 <= cap,
        "PriceAboveCap",
        format!(
            "The price can be at most {} USDC (face value + {})",
            format::usdc(u64::try_from(cap).unwrap_or(u64::MAX)),
            format::percent(event.markup_cap_bps)
        ),
    )
}

/// `delist_ticket`.
pub fn delist(ticket: &TicketState, signer: &Pubkey) -> Check {
    let seller = ticket.seller.ok_or(Violation {
        rule: "NotListed",
        message: "The ticket isn't listed".into(),
    })?;
    rule(
        seller == *signer,
        "NotSeller",
        "Only the wallet that listed the ticket can delist it",
    )
}

/// `buy_ticket`. `seller` is the seller account the transaction passes.
pub fn buy(
    event: &EventConfig,
    ticket: &TicketState,
    asset_owner: &Pubkey,
    buyer: &Pubkey,
    seller: &Pubkey,
    max_price: u64,
    now: i64,
) -> Check {
    rule(
        buyer != seller,
        "BuyerIsSeller",
        "You can't buy your own listing",
    )?;
    not_organizer(
        event,
        buyer,
        "Organizers can't buy their own event's tickets (the royalty would pay themselves)",
    )?;
    not_organizer(
        event,
        seller,
        "A listing by the organizer can't be bought (the royalty would pay the seller)",
    )?;
    rule(!event.cancelled, "EventCancelled", "The event is cancelled")?;
    rule(
        now < event.sales_end,
        "SalesEnded",
        "Ticket sales have ended",
    )?;
    let listed_seller = ticket.seller.ok_or(Violation {
        rule: "NotListed",
        message: "The ticket isn't listed".into(),
    })?;
    rule(
        listed_seller == *seller,
        "NotSeller",
        "The seller doesn't match the listing",
    )?;
    rule(
        ticket.resale_count < MAX_RESALE,
        "ResaleLimitReached",
        format!("A ticket can be resold at most {MAX_RESALE} times"),
    )?;
    rule(
        asset_owner == seller,
        "NotTicketOwner",
        "The listing is stale: the seller no longer holds the ticket",
    )?;
    rule(
        ticket.list_price <= max_price,
        "PriceExceedsMax",
        format!(
            "The listing costs {} USDC, above your maximum of {} USDC",
            format::usdc(ticket.list_price),
            format::usdc(max_price)
        ),
    )
}

/// How a resale price is paid out: no protocol fee, the royalty to the organizer and the
/// remainder to the seller, so the parts always add up to the price.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split {
    pub royalty: u64,
    pub seller: u64,
}

pub fn split(event: &EventConfig, price: u64) -> Result<Split, Violation> {
    let overflow = || Violation {
        rule: "MathOverflow",
        message: "The royalty doesn't fit in a u64".into(),
    };
    let royalty =
        u64::try_from(price as u128 * event.royalty_bps as u128 / BPS_DENOMINATOR as u128)
            .map_err(|_| overflow())?;
    let seller = price.checked_sub(royalty).ok_or_else(overflow)?;
    Ok(Split { royalty, seller })
}

/// `redeem_ticket`. `user` is the pass's holder and `asset_owner` the ticket's owner now.
pub fn redeem(
    event: &EventConfig,
    scanner: &Pubkey,
    asset_owner: &Pubkey,
    user: &Pubkey,
    expiry: i64,
    now: i64,
) -> Check {
    rule(
        event.scanners.contains(scanner),
        "InvalidScanner",
        "This wallet isn't a scanner of the event",
    )?;
    rule(!event.cancelled, "EventCancelled", "The event is cancelled")?;
    rule(now < event.event_end, "EventEnded", "The event has ended")?;
    rule(
        now <= expiry,
        "ChallengeExpired",
        "The pass has expired: ask the holder for a fresh one",
    )?;
    rule(
        expiry <= now.saturating_add(CHALLENGE_VALIDITY_SECONDS),
        "InvalidChallenge",
        format!("A pass can be valid for at most {CHALLENGE_VALIDITY_SECONDS}s"),
    )?;
    rule(
        asset_owner == user,
        "NotTicketOwner",
        "The pass holder no longer owns the ticket",
    )
}

/// `refund_ticket`.
pub fn refund(event: &EventConfig, signer: &Pubkey, asset_owner: &Pubkey, user: &Pubkey) -> Check {
    organizer(event, signer)?;
    rule(
        event.cancelled,
        "EventNotCancelled",
        "Refunds are only possible after the event is cancelled",
    )?;
    rule(
        asset_owner == user,
        "NotTicketOwner",
        "The refund must go to the ticket's current owner",
    )?;
    not_organizer(
        event,
        user,
        "You hold this ticket yourself: there's nothing to refund",
    )
}

/// `close_ticket`.
pub fn close(event: &EventConfig, asset_owner: &Pubkey, user: &Pubkey, now: i64) -> Check {
    rule(
        now >= event.event_end,
        "EventNotEnded",
        format!(
            "Tickets can be closed after the event ends ({})",
            format::relative(event.event_end, now)
        ),
    )?;
    rule(
        asset_owner == user,
        "NotTicketOwner",
        "You don't own this ticket",
    )
}

/// The app's own check: `who` has a USDC account holding at least `needed`.
pub fn usdc_funds(balance: Option<u64>, needed: u64, who: &str) -> Check {
    let balance = balance.ok_or(Violation {
        rule: "NoUsdcAccount",
        message: format!("{who} has no USDC account"),
    })?;
    rule(
        balance >= needed,
        "InsufficientUsdc",
        format!(
            "{who} needs {} USDC but has {} USDC",
            format::usdc(needed),
            format::usdc(balance)
        ),
    )
}

/// The app's own check: a USDC account the instruction pays into already exists (the program
/// doesn't create it, so the transaction would fail).
pub fn usdc_account(exists: bool, who: &str) -> Check {
    rule(
        exists,
        "NoUsdcAccount",
        format!("{who} has no USDC account to receive the payment"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(now: i64) -> EventConfig {
        EventConfig {
            event_id: 1,
            organizer: Pubkey::new_unique(),
            collection: Pubkey::new_unique(),
            ticket_price: 10_000_000,
            ticket_supply: 2,
            tickets_minted: 0,
            markup_cap_bps: 1_000,
            royalty_bps: 500,
            sales_end: now + 100,
            event_end: now + 200,
            cancelled: false,
            scanners: vec![Pubkey::new_unique()],
            bump: 255,
        }
    }

    fn ticket() -> TicketState {
        TicketState {
            event: Pubkey::new_unique(),
            resale_count: 0,
            seller: None,
            list_price: 0,
            bump: 255,
        }
    }

    #[test]
    fn listing_price_is_capped_from_face_value_and_resales_split_exactly() {
        let now = 1_000;
        let config = event(now);
        let holder = Pubkey::new_unique();

        // 10 USDC + 10% markup: 11 USDC passes, one base unit more doesn't; 0 is allowed
        assert_eq!(resale_cap(&config), 11_000_000);
        assert!(list(&config, &ticket(), &holder, &holder, 11_000_000, now).is_ok());
        assert!(list(&config, &ticket(), &holder, &holder, 0, now).is_ok());
        let over = list(&config, &ticket(), &holder, &holder, 11_000_001, now).unwrap_err();
        assert_eq!(over.rule, "PriceAboveCap");

        // Only the holder can list, once, and not after sales close
        let stranger = Pubkey::new_unique();
        assert_eq!(
            list(&config, &ticket(), &holder, &stranger, 1, now)
                .unwrap_err()
                .rule,
            "NotTicketOwner"
        );
        let listed = TicketState {
            seller: Some(holder),
            list_price: 1,
            ..ticket()
        };
        assert_eq!(
            list(&config, &listed, &holder, &holder, 1, now)
                .unwrap_err()
                .rule,
            "AlreadyListed"
        );
        assert_eq!(
            list(&config, &ticket(), &holder, &holder, 1, now + 100)
                .unwrap_err()
                .rule,
            "SalesEnded"
        );

        // The split always sums to the price; the seller takes the rounding remainder
        for price in [0, 1, 19, 11_000_000, u64::MAX] {
            let parts = split(&config, price).unwrap();
            assert_eq!(parts.royalty + parts.seller, price);
        }
        assert_eq!(split(&config, 11_000_000).unwrap().royalty, 550_000);
    }

    #[test]
    fn buy_rejects_own_stale_and_overpriced_listings() {
        let now = 1_000;
        let config = event(now);
        let (seller, buyer) = (Pubkey::new_unique(), Pubkey::new_unique());
        let listed = TicketState {
            seller: Some(seller),
            list_price: 5,
            ..ticket()
        };
        assert!(buy(&config, &listed, &seller, &buyer, &seller, 5, now).is_ok());
        let fails = |owner: &Pubkey, buyer: &Pubkey, max: u64| {
            buy(&config, &listed, owner, buyer, &seller, max, now)
                .unwrap_err()
                .rule
        };
        assert_eq!(fails(&seller, &seller, 5), "BuyerIsSeller");
        assert_eq!(fails(&Pubkey::new_unique(), &buyer, 5), "NotTicketOwner");
        assert_eq!(fails(&seller, &buyer, 4), "PriceExceedsMax");
        let resold = TicketState {
            resale_count: MAX_RESALE,
            ..listed
        };
        assert_eq!(
            buy(&config, &resold, &seller, &buyer, &seller, 5, now)
                .unwrap_err()
                .rule,
            "ResaleLimitReached"
        );
    }

    #[test]
    fn lifecycle_gates_follow_the_cluster_clock() {
        let now = 1_000;
        let mut config = event(now);
        let scanner = config.scanners[0];
        let holder = Pubkey::new_unique();

        // Mint: limit per wallet, supply
        let minter = Pubkey::new_unique();
        assert!(mint(&config, &minter, MAX_MINTS_PER_WALLET - 1, now).is_ok());
        assert_eq!(
            mint(&config, &config.organizer, 0, now).unwrap_err().rule,
            "ConstraintDuplicateMutableAccount"
        );
        assert_eq!(
            mint(&config, &minter, MAX_MINTS_PER_WALLET, now)
                .unwrap_err()
                .rule,
            "MintLimitReached"
        );
        config.tickets_minted = config.ticket_supply;
        assert_eq!(
            mint(&config, &minter, 0, now).unwrap_err().rule,
            "EventSoldOut"
        );

        // Redeem keeps working after sales close, until the event ends; passes live ≤ 90s
        let after_sales = now + 150;
        assert!(redeem(
            &config,
            &scanner,
            &holder,
            &holder,
            after_sales + 60,
            after_sales
        )
        .is_ok());
        assert_eq!(
            redeem(
                &config,
                &scanner,
                &holder,
                &holder,
                after_sales + 91,
                after_sales
            )
            .unwrap_err()
            .rule,
            "InvalidChallenge"
        );
        assert_eq!(
            redeem(
                &config,
                &scanner,
                &holder,
                &holder,
                after_sales - 1,
                after_sales
            )
            .unwrap_err()
            .rule,
            "ChallengeExpired"
        );
        assert_eq!(
            redeem(&config, &holder, &holder, &holder, after_sales, after_sales)
                .unwrap_err()
                .rule,
            "InvalidScanner"
        );
        assert_eq!(
            close(&config, &holder, &holder, after_sales)
                .unwrap_err()
                .rule,
            "EventNotEnded"
        );
        assert!(close(&config, &holder, &holder, now + 200).is_ok());

        // Cancellation blocks redeem and enables refunds, for the organizer only
        let organizer = config.organizer;
        assert_eq!(
            refund(&config, &organizer, &holder, &holder)
                .unwrap_err()
                .rule,
            "EventNotCancelled"
        );
        config.cancelled = true;
        assert!(refund(&config, &organizer, &holder, &holder).is_ok());
        assert_eq!(
            refund(&config, &holder, &holder, &holder).unwrap_err().rule,
            "NotOrganizer"
        );
        assert_eq!(
            redeem(&config, &scanner, &holder, &holder, now, now)
                .unwrap_err()
                .rule,
            "EventCancelled"
        );
        assert_eq!(
            cancel(&config, &organizer).unwrap_err().rule,
            "EventCancelled"
        );
    }
}
