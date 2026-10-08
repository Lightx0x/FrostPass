use anchor_lang::{
    prelude::*,
    system_program::{transfer, Transfer},
};
use mpl_core::{instructions::BurnV1CpiBuilder, ID as MPL_CORE_ID};
use solana_instructions_sysvar::get_instruction_relative;

use crate::{
    constants::{
        CHALLENGE_DOMAIN, CHALLENGE_NONCE_LENGTH, CHALLENGE_VALIDITY_SECONDS, EVENT_SEED,
        TICKET_SEED,
    },
    error::ErrorCode,
    state::{EventConfig, TicketState},
    utils::load_ticket_asset,
};

const ED25519_PROGRAM_ID: Pubkey = pubkey!("Ed25519SigVerify111111111111111111111111111");
const ED25519_HEADER_LENGTH: usize = 16;
const ED25519_SIGNATURE_LENGTH: usize = 64;
const ED25519_PUBLIC_KEY_LENGTH: usize = 32;
const CURRENT_INSTRUCTION_INDEX: u16 = u16::MAX;
const CHALLENGE_MESSAGE_LENGTH: usize = 136;

#[derive(Accounts)]
pub struct RedeemTicket<'info> {
    #[account(mut)]
    pub scanner: Signer<'info>,

    #[account(mut)]
    pub user: SystemAccount<'info>,

    #[account(
        seeds = [
            EVENT_SEED,
            event_config.organizer.as_ref(),
            event_config.event_id.to_le_bytes().as_ref(),
        ],
        bump = event_config.bump,
    )]
    pub event_config: Account<'info, EventConfig>,

    /// CHECK: Address and MPL Core ownership are constrained.
    #[account(
        mut,
        address = event_config.collection @ ErrorCode::InvalidCollection,
        owner = MPL_CORE_ID,
    )]
    pub collection: UncheckedAccount<'info>,

    /// CHECK: MPL Core ownership is constrained; asset data is checked in the handler.
    #[account(mut, owner = MPL_CORE_ID)]
    pub ticket_asset: UncheckedAccount<'info>,

    #[account(
        mut,
        close = user,
        seeds = [TICKET_SEED, ticket_asset.key().as_ref()],
        bump = ticket_state.bump,
        constraint = ticket_state.event == event_config.key()
            @ ErrorCode::InvalidEvent,
    )]
    pub ticket_state: Account<'info, TicketState>,

    /// CHECK: Constrained to the instructions sysvar.
    #[account(address = solana_instructions_sysvar::ID)]
    pub instructions_sysvar: UncheckedAccount<'info>,

    /// CHECK: Constrained to the MPL Core program.
    #[account(address = MPL_CORE_ID)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handle_redeem_ticket(
    ctx: Context<RedeemTicket>,
    // Freshness only — replay protection comes from burn + expiry
    nonce: [u8; CHALLENGE_NONCE_LENGTH],
    expiry: i64,
) -> Result<()> {
    let event_config = &ctx.accounts.event_config;
    let current_time = Clock::get()?.unix_timestamp;

    require!(
        event_config.scanners.contains(&ctx.accounts.scanner.key()),
        ErrorCode::InvalidScanner
    );

    require!(current_time < event_config.event_end, ErrorCode::EventEnded);

    require!(current_time <= expiry, ErrorCode::ChallengeExpired);

    let maximum_expiry = current_time
        .checked_add(CHALLENGE_VALIDITY_SECONDS)
        .ok_or(ErrorCode::MathOverflow)?;

    require!(expiry <= maximum_expiry, ErrorCode::InvalidChallenge);

    require!(
        ctx.accounts.ticket_state.seller.is_none(),
        ErrorCode::TicketListed
    );

    let asset = load_ticket_asset(&ctx.accounts.ticket_asset, event_config)?;

    require_keys_eq!(
        asset.owner,
        ctx.accounts.user.key(),
        ErrorCode::NotTicketOwner
    );

    let message = build_challenge_message(
        &ctx.accounts.event_config.key(),
        &ctx.accounts.ticket_asset.key(),
        &ctx.accounts.user.key(),
        &nonce,
        expiry,
    );

    verify_ed25519_instruction(
        &ctx.accounts.instructions_sysvar.to_account_info(),
        &ctx.accounts.user.key(),
        &message,
    )?;

    let event_id_bytes = event_config.event_id.to_le_bytes();
    let bump_seed = [event_config.bump];

    let signer_seeds: &[&[u8]] = &[
        EVENT_SEED,
        event_config.organizer.as_ref(),
        event_id_bytes.as_ref(),
        &bump_seed,
    ];

    // mpl-core refunds the burned asset's rent to the burn payer (the scanner)
    let scanner_lamports_before = ctx.accounts.scanner.lamports();

    BurnV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .asset(&ctx.accounts.ticket_asset.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .payer(&ctx.accounts.scanner.to_account_info())
        .authority(Some(&ctx.accounts.event_config.to_account_info()))
        .system_program(Some(&ctx.accounts.system_program.to_account_info()))
        .invoke_signed(&[signer_seeds])?;

    // Forward that refund to `user`, the ticket's last owner before redeem
    // (does not have to be the original minter)
    let asset_rent_refund = ctx
        .accounts
        .scanner
        .lamports()
        .saturating_sub(scanner_lamports_before);

    if asset_rent_refund > 0 {
        transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                Transfer {
                    from: ctx.accounts.scanner.to_account_info(),
                    to: ctx.accounts.user.to_account_info(),
                },
            ),
            asset_rent_refund,
        )?;
    }

    Ok(())
}

fn build_challenge_message(
    event: &Pubkey,
    asset: &Pubkey,
    user: &Pubkey,
    nonce: &[u8; CHALLENGE_NONCE_LENGTH],
    expiry: i64,
) -> [u8; CHALLENGE_MESSAGE_LENGTH] {
    let mut message = [0u8; CHALLENGE_MESSAGE_LENGTH];

    message[0..16].copy_from_slice(CHALLENGE_DOMAIN);
    message[16..48].copy_from_slice(event.as_ref());
    message[48..80].copy_from_slice(asset.as_ref());
    message[80..112].copy_from_slice(user.as_ref());
    message[112..128].copy_from_slice(nonce);
    message[128..136].copy_from_slice(&expiry.to_le_bytes());

    message
}

fn verify_ed25519_instruction(
    instructions_sysvar: &AccountInfo,
    expected_public_key: &Pubkey,
    expected_message: &[u8],
) -> Result<()> {
    let instruction = get_instruction_relative(-1, instructions_sysvar)
        .map_err(|_| ErrorCode::InvalidChallenge)?;

    require_keys_eq!(
        instruction.program_id,
        ED25519_PROGRAM_ID,
        ErrorCode::InvalidChallenge
    );

    let data = &instruction.data;

    require!(
        data.len() >= ED25519_HEADER_LENGTH,
        ErrorCode::InvalidChallenge
    );
    require!(data[0] == 1, ErrorCode::InvalidChallenge);
    require!(data[1] == 0, ErrorCode::InvalidChallenge);

    let signature_offset = read_u16(data, 2)? as usize;
    let signature_instruction_index = read_u16(data, 4)?;
    let public_key_offset = read_u16(data, 6)? as usize;
    let public_key_instruction_index = read_u16(data, 8)?;
    let message_offset = read_u16(data, 10)? as usize;
    let message_length = read_u16(data, 12)? as usize;
    let message_instruction_index = read_u16(data, 14)?;

    require!(
        signature_instruction_index == CURRENT_INSTRUCTION_INDEX,
        ErrorCode::InvalidChallenge
    );
    require!(
        public_key_instruction_index == CURRENT_INSTRUCTION_INDEX,
        ErrorCode::InvalidChallenge
    );
    require!(
        message_instruction_index == CURRENT_INSTRUCTION_INDEX,
        ErrorCode::InvalidChallenge
    );

    checked_slice(data, signature_offset, ED25519_SIGNATURE_LENGTH)?;

    let public_key = checked_slice(data, public_key_offset, ED25519_PUBLIC_KEY_LENGTH)?;

    require!(
        public_key == expected_public_key.as_ref(),
        ErrorCode::InvalidChallenge
    );

    require!(
        message_length == expected_message.len(),
        ErrorCode::InvalidChallenge
    );

    let message = checked_slice(data, message_offset, message_length)?;

    require!(message == expected_message, ErrorCode::InvalidChallenge);

    Ok(())
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    let first = *data.get(offset).ok_or(ErrorCode::InvalidChallenge)?;
    let second = *data.get(offset + 1).ok_or(ErrorCode::InvalidChallenge)?;

    Ok(u16::from_le_bytes([first, second]))
}

fn checked_slice(data: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(length)
        .ok_or(ErrorCode::InvalidChallenge)?;

    match data.get(offset..end) {
        Some(slice) => Ok(slice),
        None => err!(ErrorCode::InvalidChallenge),
    }
}
