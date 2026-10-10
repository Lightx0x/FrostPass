//! `list_ticket` / `buy_ticket`: test plan #6 and #7.

mod common;

use common::*;
use frost_pass::{
    constants::{MINT_FEE, PROTOCOL_TREASURY},
    error::ErrorCode,
};
use solana_keypair::Keypair;
use solana_signer::Signer;

/// Highest allowed resale price for the default event: face value plus the 10% markup cap.
const CAP_PRICE: u64 = TICKET_PRICE * (10_000 + MARKUP_CAP_BPS as u64) / 10_000;
/// Organizer royalty on a resale at `CAP_PRICE` (5%).
const ROYALTY: u64 = CAP_PRICE * ROYALTY_BPS as u64 / 10_000;

/// #6: a resale at the cap pays the royalty to the organizer and the rest to the seller (no
/// protocol fee), moves the ticket to the buyer still frozen, and clears the listing.
#[test]
fn resale_pays_royalty_and_seller_and_moves_frozen_ticket() {
    let mut ctx = TestContext::new();
    let event = ctx.create_event(InitEventParams::default());
    let seller = ctx.funded_wallet();
    let buyer = ctx.funded_wallet();
    let ticket = ctx.mint_ticket(&event, &seller);

    ctx.list_ticket(&event, &seller, &ticket, CAP_PRICE);
    expect_success(ctx.try_buy_ticket(&event, &buyer, &seller.pubkey(), &ticket, CAP_PRICE));

    // Payments: the buyer pays the list price, split between seller and organizer only.
    assert_eq!(ctx.usdc_balance(&buyer.pubkey()), WALLET_USDC - CAP_PRICE);
    assert_eq!(
        ctx.usdc_balance(&seller.pubkey()),
        WALLET_USDC - TICKET_PRICE - MINT_FEE + (CAP_PRICE - ROYALTY)
    );
    assert_eq!(ctx.usdc_balance(&event.organizer), TICKET_PRICE + ROYALTY);
    assert_eq!(
        ctx.usdc_balance(&PROTOCOL_TREASURY),
        MINT_FEE,
        "resales pay no protocol fee"
    );

    // The ticket now belongs to the buyer and is still frozen.
    let asset = ctx.ticket_asset(&ticket).expect("ticket should exist");
    assert_eq!(asset.owner, buyer.pubkey());
    let friend = Keypair::new();
    let logs =
        expect_failure(ctx.try_direct_core_transfer(&event, &buyer, &ticket, &friend.pubkey()));
    assert_blocked_by_freeze(&logs);

    // The listing is cleared and the resale counted.
    let ticket_state = ctx.ticket_state(&ticket).unwrap();
    assert_eq!(ticket_state.seller, None);
    assert_eq!(ticket_state.list_price, 0);
    assert_eq!(ticket_state.resale_count, 1);
}

/// #7: a purchase fails when the ticket isn't listed, when the `seller` account isn't the
/// listed seller, when the price is above the buyer's `max_price`, and when the listed seller no
/// longer holds the ticket. Nothing moves in any of these cases.
#[test]
fn buy_ticket_rejects_invalid_purchases() {
    let mut ctx = TestContext::new();
    let event = ctx.create_event(InitEventParams::default());
    let seller = ctx.funded_wallet();
    let buyer = ctx.funded_wallet();
    let impostor = ctx.funded_wallet();
    let ticket = ctx.mint_ticket(&event, &seller);

    // Not listed
    let logs =
        expect_failure(ctx.try_buy_ticket(&event, &buyer, &seller.pubkey(), &ticket, CAP_PRICE));
    assert_custom_error(&logs, ErrorCode::NotListed);

    ctx.list_ticket(&event, &seller, &ticket, CAP_PRICE);

    // Wrong `seller` account: payment would go to someone who didn't list the ticket
    let logs =
        expect_failure(ctx.try_buy_ticket(&event, &buyer, &impostor.pubkey(), &ticket, CAP_PRICE));
    assert_custom_error(&logs, ErrorCode::NotSeller);

    // Listing price above the buyer's slippage limit
    let logs = expect_failure(ctx.try_buy_ticket(
        &event,
        &buyer,
        &seller.pubkey(),
        &ticket,
        CAP_PRICE - 1,
    ));
    assert_custom_error(&logs, ErrorCode::PriceExceedsMax);

    // Stale listing: the listed seller no longer owns the ticket
    ctx.force_asset_owner(&ticket, &impostor.pubkey());
    let logs =
        expect_failure(ctx.try_buy_ticket(&event, &buyer, &seller.pubkey(), &ticket, CAP_PRICE));
    assert_custom_error(&logs, ErrorCode::NotTicketOwner);

    // No purchase went through.
    assert_eq!(ctx.usdc_balance(&buyer.pubkey()), WALLET_USDC);
    assert_eq!(ctx.ticket_state(&ticket).unwrap().resale_count, 0);
    assert_eq!(
        ctx.ticket_asset(&ticket).unwrap().owner,
        impostor.pubkey(),
        "the ticket must not have moved to the buyer"
    );
}
