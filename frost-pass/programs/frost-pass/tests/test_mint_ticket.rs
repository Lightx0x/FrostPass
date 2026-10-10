//! `mint_ticket` and the freeze: test plan #4, #5 and #15.

mod common;

use common::*;
use frost_pass::{
    constants::{MAX_MINTS_PER_WALLET, MINT_FEE, PROTOCOL_TREASURY},
    error::ErrorCode,
};
use mpl_core::types::UpdateAuthority;
use solana_keypair::Keypair;
use solana_signer::Signer;

/// #4: the minter pays `ticket_price + MINT_FEE` (organizer gets the full price, treasury the
/// fee) and receives a ticket inside the frozen event collection, with its state initialized.
#[test]
fn mint_ticket_pays_organizer_and_treasury_and_issues_frozen_ticket() {
    let mut ctx = TestContext::new();
    let event = ctx.create_event(InitEventParams::default());
    let minter = ctx.funded_wallet();

    let ticket = ctx.mint_ticket(&event, &minter);

    // Payments
    assert_eq!(
        ctx.usdc_balance(&minter.pubkey()),
        WALLET_USDC - TICKET_PRICE - MINT_FEE
    );
    assert_eq!(ctx.usdc_balance(&event.organizer), TICKET_PRICE);
    assert_eq!(ctx.usdc_balance(&PROTOCOL_TREASURY), MINT_FEE);

    // The ticket: owned by the minter, in this event's collection, named by the program
    let asset = ctx.ticket_asset(&ticket).expect("ticket should exist");
    assert_eq!(asset.owner, minter.pubkey());
    assert_eq!(
        asset.update_authority,
        UpdateAuthority::Collection(event.collection)
    );
    assert_eq!(asset.name, format!("{EVENT_NAME} #1"));
    assert_eq!(asset.uri, EVENT_URI);
    assert!(
        ctx.collection_is_frozen(&event),
        "the collection's freeze must cover the new ticket"
    );

    // State
    let ticket_state = ctx.ticket_state(&ticket).expect("TicketState should exist");
    assert_eq!(ticket_state.event, event.config);
    assert_eq!(ticket_state.resale_count, 0);
    assert_eq!(ticket_state.seller, None);
    assert_eq!(ticket_state.list_price, 0);

    assert_eq!(ctx.event_config(&event).tickets_minted, 1);
    let record = ctx
        .minter_record(&event, &minter.pubkey())
        .expect("MinterRecord should exist");
    assert_eq!(record.count, 1);
}

/// #5: the holder can't move their ticket by calling Metaplex Core directly; the collection's
/// freeze rejects it, so tickets only move through FrostPass.
#[test]
fn holder_cannot_transfer_ticket_directly() {
    let mut ctx = TestContext::new();
    let event = ctx.create_event(InitEventParams::default());
    let holder = ctx.funded_wallet();
    let ticket = ctx.mint_ticket(&event, &holder);
    let friend = Keypair::new();

    let logs =
        expect_failure(ctx.try_direct_core_transfer(&event, &holder, &ticket, &friend.pubkey()));
    assert_blocked_by_freeze(&logs);

    let asset = ctx
        .ticket_asset(&ticket)
        .expect("ticket should still exist");
    assert_eq!(
        asset.owner,
        holder.pubkey(),
        "the ticket must not have moved"
    );
}

/// #15: a wallet can mint `MAX_MINTS_PER_WALLET` tickets per event and no more; other wallets
/// are unaffected.
#[test]
fn mint_limit_is_enforced_per_wallet() {
    let mut ctx = TestContext::new();
    let event = ctx.create_event(InitEventParams::default());
    let minter = ctx.funded_wallet();

    for _ in 0..MAX_MINTS_PER_WALLET {
        ctx.mint_ticket(&event, &minter);
    }
    let logs = expect_failure(ctx.try_mint_ticket(&event, &minter));
    assert_custom_error(&logs, ErrorCode::MintLimitReached);

    let record = ctx.minter_record(&event, &minter.pubkey()).unwrap();
    assert_eq!(record.count, MAX_MINTS_PER_WALLET);

    let other = ctx.funded_wallet();
    ctx.mint_ticket(&event, &other);
    assert_eq!(
        ctx.event_config(&event).tickets_minted,
        u32::from(MAX_MINTS_PER_WALLET) + 1
    );
}
