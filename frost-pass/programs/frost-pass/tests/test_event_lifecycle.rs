//! Cancellation, refunds, closing and the event's time windows: test plan #10, #12 and #14.

mod common;

use anchor_lang::error::ErrorCode as AnchorErrorCode;
use common::*;
use frost_pass::error::ErrorCode;
use solana_signer::Signer;

/// #10: refunds only work on a cancelled event and only for the organizer. The current owner
/// (here a resale buyer) gets face value, the ticket is burned, and both rents go to the owner
/// while the organizer only pays the transaction fee.
#[test]
fn refund_pays_face_value_to_current_owner_on_cancelled_event() {
    let mut ctx = TestContext::new();
    let event = ctx.create_event(InitEventParams::default());
    let organizer = ctx.organizer.insecure_clone();
    let minter = ctx.funded_wallet();
    let owner = ctx.funded_wallet();
    let ticket = ctx.mint_ticket(&event, &minter);

    // Resell above face value so the current owner differs from the minter.
    let resale_price = TICKET_PRICE + ONE_USDC;
    ctx.list_ticket(&event, &minter, &ticket, resale_price);
    expect_success(ctx.try_buy_ticket(&event, &owner, &minter.pubkey(), &ticket, resale_price));

    // Not cancelled yet
    let logs = expect_failure(ctx.try_refund_ticket(&event, &organizer, &owner.pubkey(), &ticket));
    assert_custom_error(&logs, ErrorCode::EventNotCancelled);

    expect_success(ctx.try_cancel_event(&event, &organizer));
    assert!(ctx.event_config(&event).cancelled);

    // Only the organizer can refund (a non-organizer fails the EventConfig seeds check).
    let attacker = ctx.funded_wallet();
    let logs = expect_failure(ctx.try_refund_ticket(&event, &attacker, &owner.pubkey(), &ticket));
    assert_anchor_error(&logs, AnchorErrorCode::ConstraintSeeds);

    // Only the current owner can be refunded.
    let logs = expect_failure(ctx.try_refund_ticket(&event, &organizer, &minter.pubkey(), &ticket));
    assert_custom_error(&logs, ErrorCode::NotTicketOwner);

    let owner_usdc = ctx.usdc_balance(&owner.pubkey());
    let organizer_usdc = ctx.usdc_balance(&event.organizer);
    let owner_lamports = ctx.lamports(&owner.pubkey());
    let organizer_lamports = ctx.lamports(&event.organizer);
    let asset_before = ctx.lamports(&ticket);
    let ticket_state_rent = ctx.lamports(&ticket_state_pda(&ticket));

    let meta = expect_success(ctx.try_refund_ticket(&event, &organizer, &owner.pubkey(), &ticket));

    // Face value, not the resale price
    assert_eq!(ctx.usdc_balance(&owner.pubkey()), owner_usdc + TICKET_PRICE);
    assert_eq!(
        ctx.usdc_balance(&event.organizer),
        organizer_usdc - TICKET_PRICE
    );

    assert!(
        ctx.ticket_asset(&ticket).is_none(),
        "the ticket must be burned"
    );
    assert!(
        ctx.ticket_state(&ticket).is_none(),
        "the TicketState must be closed"
    );

    let asset_refund = asset_before - ctx.lamports(&ticket);
    assert_eq!(
        ctx.lamports(&owner.pubkey()),
        owner_lamports + asset_refund + ticket_state_rent
    );
    assert_eq!(
        ctx.lamports(&event.organizer),
        organizer_lamports - meta.fee
    );
}

/// #12: `close_ticket` only works after `event_end` and only for the ticket's owner, who gets
/// both rents back.
#[test]
fn close_ticket_only_by_owner_after_event_end() {
    let mut ctx = TestContext::new();
    let event = ctx.create_event(InitEventParams::default());
    let holder = ctx.funded_wallet();
    let stranger = ctx.funded_wallet();
    let ticket = ctx.mint_ticket(&event, &holder);

    let logs = expect_failure(ctx.try_close_ticket(&event, &holder, &ticket));
    assert_custom_error(&logs, ErrorCode::EventNotEnded);

    ctx.set_time(EVENT_END);

    let logs = expect_failure(ctx.try_close_ticket(&event, &stranger, &ticket));
    assert_custom_error(&logs, ErrorCode::NotTicketOwner);

    let holder_before = ctx.lamports(&holder.pubkey());
    let asset_before = ctx.lamports(&ticket);
    let ticket_state_rent = ctx.lamports(&ticket_state_pda(&ticket));

    let meta = expect_success(ctx.try_close_ticket(&event, &holder, &ticket));

    assert!(
        ctx.ticket_asset(&ticket).is_none(),
        "the ticket must be burned"
    );
    assert!(
        ctx.ticket_state(&ticket).is_none(),
        "the TicketState must be closed"
    );
    let asset_refund = asset_before - ctx.lamports(&ticket);
    assert_eq!(
        ctx.lamports(&holder.pubkey()),
        holder_before + asset_refund + ticket_state_rent - meta.fee
    );
}

/// #14: once `sales_end` passes, minting and buying stop but redeeming still works until
/// `event_end`; once an event is cancelled, minting, listing, buying and redeeming all stop.
#[test]
fn sales_end_and_cancellation_gate_the_right_instructions() {
    let mut ctx = TestContext::new();
    let scanner = ctx.funded_wallet();
    let organizer = ctx.organizer.insecure_clone();
    let holder = ctx.funded_wallet();
    let buyer = ctx.funded_wallet();
    let latecomer = ctx.funded_wallet();

    let open_event = ctx.create_event(InitEventParams::with_scanners(1, &[&scanner]));
    let cancelled_event = ctx.create_event(InitEventParams::with_scanners(2, &[&scanner]));

    let open_ticket = ctx.mint_ticket(&open_event, &holder);
    ctx.list_ticket(&open_event, &holder, &open_ticket, TICKET_PRICE);
    let listed_ticket = ctx.mint_ticket(&cancelled_event, &holder);
    ctx.list_ticket(&cancelled_event, &holder, &listed_ticket, TICKET_PRICE);
    let unlisted_ticket = ctx.mint_ticket(&cancelled_event, &holder);

    // Cancellation blocks mint, list, buy and redeem.
    expect_success(ctx.try_cancel_event(&cancelled_event, &organizer));

    let logs = expect_failure(ctx.try_mint_ticket(&cancelled_event, &latecomer));
    assert_custom_error(&logs, ErrorCode::EventCancelled);
    let logs = expect_failure(ctx.try_list_ticket(
        &cancelled_event,
        &holder,
        &unlisted_ticket,
        TICKET_PRICE,
    ));
    assert_custom_error(&logs, ErrorCode::EventCancelled);
    let logs = expect_failure(ctx.try_buy_ticket(
        &cancelled_event,
        &buyer,
        &holder.pubkey(),
        &listed_ticket,
        TICKET_PRICE,
    ));
    assert_custom_error(&logs, ErrorCode::EventCancelled);
    let logs = expect_failure(ctx.try_redeem_as_owner(
        &cancelled_event,
        &scanner,
        &holder,
        &unlisted_ticket,
    ));
    assert_custom_error(&logs, ErrorCode::EventCancelled);

    // After sales_end, minting and buying stop, but the holder can still get in.
    ctx.set_time(SALES_END);

    let logs = expect_failure(ctx.try_mint_ticket(&open_event, &latecomer));
    assert_custom_error(&logs, ErrorCode::SalesEnded);
    let logs = expect_failure(ctx.try_buy_ticket(
        &open_event,
        &buyer,
        &holder.pubkey(),
        &open_ticket,
        TICKET_PRICE,
    ));
    assert_custom_error(&logs, ErrorCode::SalesEnded);

    expect_success(ctx.try_redeem_as_owner(&open_event, &scanner, &holder, &open_ticket));
    assert!(ctx.ticket_asset(&open_ticket).is_none());
}
