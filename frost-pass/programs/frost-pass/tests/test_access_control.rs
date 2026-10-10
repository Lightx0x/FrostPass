//! Cross-event isolation and scanner authority: test plan #11 and #13.

mod common;

use anchor_lang::error::ErrorCode as AnchorErrorCode;
use common::*;
use frost_pass::error::ErrorCode;
use solana_signer::Signer;

/// #11: a ticket of event A can't be listed, bought or redeemed through event B's accounts, even
/// by a scanner that is authorized for event B.
#[test]
fn ticket_cannot_be_used_with_another_events_accounts() {
    let mut ctx = TestContext::new();
    let scanner_b = ctx.funded_wallet();
    let event_a = ctx.create_event(InitEventParams::with_id(1));
    let event_b = ctx.create_event(InitEventParams::with_scanners(2, &[&scanner_b]));
    let holder = ctx.funded_wallet();
    let buyer = ctx.funded_wallet();
    let ticket = ctx.mint_ticket(&event_a, &holder);

    let logs = expect_failure(ctx.try_list_ticket(&event_b, &holder, &ticket, TICKET_PRICE));
    assert_custom_error(&logs, ErrorCode::InvalidEvent);

    ctx.list_ticket(&event_a, &holder, &ticket, TICKET_PRICE);
    let logs = expect_failure(ctx.try_buy_ticket(
        &event_b,
        &buyer,
        &holder.pubkey(),
        &ticket,
        TICKET_PRICE,
    ));
    assert_custom_error(&logs, ErrorCode::InvalidEvent);

    let logs = expect_failure(ctx.try_redeem_as_owner(&event_b, &scanner_b, &holder, &ticket));
    assert_custom_error(&logs, ErrorCode::InvalidEvent);

    let asset = ctx
        .ticket_asset(&ticket)
        .expect("the ticket must not be burned");
    assert_eq!(asset.owner, holder.pubkey());
    assert_eq!(ctx.usdc_balance(&buyer.pubkey()), WALLET_USDC);
}

/// #13: only the organizer can change the scanner list, and a scanner removed from it can no
/// longer redeem tickets while the remaining one still can.
#[test]
fn only_organizer_controls_scanners_and_removed_scanner_cannot_redeem() {
    let mut ctx = TestContext::new();
    let scanner_1 = ctx.funded_wallet();
    let scanner_2 = ctx.funded_wallet();
    let event = ctx.create_event(InitEventParams::with_scanners(1, &[&scanner_1, &scanner_2]));

    // A non-organizer signer derives a different EventConfig address, so the seeds check
    // rejects it before `has_one = organizer` is reached.
    let attacker = ctx.funded_wallet();
    let logs = expect_failure(ctx.try_update_scanners(&event, &attacker, vec![attacker.pubkey()]));
    assert_anchor_error(&logs, AnchorErrorCode::ConstraintSeeds);
    assert_eq!(
        ctx.event_config(&event).scanners,
        vec![scanner_1.pubkey(), scanner_2.pubkey()]
    );

    // The organizer removes scanner 1.
    let organizer = ctx.organizer.insecure_clone();
    expect_success(ctx.try_update_scanners(&event, &organizer, vec![scanner_2.pubkey()]));
    assert_eq!(ctx.event_config(&event).scanners, vec![scanner_2.pubkey()]);

    let holder = ctx.funded_wallet();
    let ticket = ctx.mint_ticket(&event, &holder);

    let logs = expect_failure(ctx.try_redeem_as_owner(&event, &scanner_1, &holder, &ticket));
    assert_custom_error(&logs, ErrorCode::InvalidScanner);

    expect_success(ctx.try_redeem_as_owner(&event, &scanner_2, &holder, &ticket));
    assert!(ctx.ticket_asset(&ticket).is_none());
}
