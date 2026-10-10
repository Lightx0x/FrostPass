//! `redeem_ticket`: test plan #8 and #9.

mod common;

use common::*;
use frost_pass::{constants::CHALLENGE_NONCE_LENGTH, error::ErrorCode};
use solana_keypair::Keypair;
use solana_signer::Signer;

/// #8: with a valid owner-signed challenge, the ticket is burned, its `TicketState` closed, and
/// both rents (the asset's refund and the `TicketState` rent) go to the owner. The scanner only
/// pays the transaction fee.
#[test]
fn redeem_burns_ticket_and_returns_both_rents_to_owner() {
    let mut ctx = TestContext::new();
    let scanner = ctx.funded_wallet();
    let event = ctx.create_event(InitEventParams::with_scanners(1, &[&scanner]));
    let holder = ctx.funded_wallet();
    let ticket = ctx.mint_ticket(&event, &holder);

    let holder_before = ctx.lamports(&holder.pubkey());
    let scanner_before = ctx.lamports(&scanner.pubkey());
    let asset_before = ctx.lamports(&ticket);
    let ticket_state_rent = ctx.lamports(&ticket_state_pda(&ticket));

    let meta = expect_success(ctx.try_redeem_as_owner(&event, &scanner, &holder, &ticket));

    assert!(
        ctx.ticket_asset(&ticket).is_none(),
        "the ticket must be burned"
    );
    assert!(
        ctx.ticket_state(&ticket).is_none(),
        "the TicketState must be closed"
    );

    // mpl-core keeps a small balance in the burned asset's tombstone; the rest is refunded.
    let asset_refund = asset_before - ctx.lamports(&ticket);
    assert_eq!(
        ctx.lamports(&holder.pubkey()),
        holder_before + asset_refund + ticket_state_rent,
        "the owner must receive the asset refund and the TicketState rent"
    );
    assert_eq!(
        ctx.lamports(&scanner.pubkey()),
        scanner_before - meta.fee,
        "the scanner must only pay the transaction fee"
    );
}

/// #9: redemption fails for a scanner that isn't on the event's list, for a challenge signed by
/// someone other than the owner, for a signature over a different challenge, and for an expired
/// challenge. The ticket survives all of them.
#[test]
fn redeem_rejects_invalid_scanner_signature_and_challenge() {
    let mut ctx = TestContext::new();
    let scanner = ctx.funded_wallet();
    let event = ctx.create_event(InitEventParams::with_scanners(1, &[&scanner]));
    let holder = ctx.funded_wallet();
    let ticket = ctx.mint_ticket(&event, &holder);
    ctx.set_time(1_000);

    // Scanner not on the event's list
    let outsider = ctx.funded_wallet();
    let logs = expect_failure(ctx.try_redeem_as_owner(&event, &outsider, &holder, &ticket));
    assert_custom_error(&logs, ErrorCode::InvalidScanner);

    // Valid signature, but by someone other than the ticket's owner
    let attacker = Keypair::new();
    let challenge = RedeemChallenge::new(&event, &ticket, &holder.pubkey(), ctx.now());
    let logs = expect_failure(ctx.try_redeem_ticket(
        &event,
        &scanner,
        &holder.pubkey(),
        &ticket,
        &challenge,
        &attacker,
    ));
    assert_custom_error(&logs, ErrorCode::InvalidChallenge);

    // The owner signed a different challenge (another nonce) than the one submitted
    let signed = RedeemChallenge {
        nonce: [9u8; CHALLENGE_NONCE_LENGTH],
        ..challenge.clone()
    };
    let ed25519_ix = signed.signed_by(&holder);
    let redeem_ix = redeem_ticket_ix(
        &event,
        &scanner.pubkey(),
        &holder.pubkey(),
        &ticket,
        &challenge,
    );
    let logs = expect_failure(ctx.send(&[ed25519_ix, redeem_ix], &[&scanner]));
    assert_custom_error(&logs, ErrorCode::InvalidChallenge);

    // Expired challenge
    let expired = RedeemChallenge {
        expiry: ctx.now() - 1,
        ..challenge.clone()
    };
    let logs = expect_failure(ctx.try_redeem_ticket(
        &event,
        &scanner,
        &holder.pubkey(),
        &ticket,
        &expired,
        &holder,
    ));
    assert_custom_error(&logs, ErrorCode::ChallengeExpired);

    let asset = ctx
        .ticket_asset(&ticket)
        .expect("the ticket must not be burned");
    assert_eq!(asset.owner, holder.pubkey());
    assert!(ctx.ticket_state(&ticket).is_some());
}
