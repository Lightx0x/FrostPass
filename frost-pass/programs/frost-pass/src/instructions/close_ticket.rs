use anchor_lang::prelude::*;
use mpl_core::ID as MPL_CORE_ID;

use crate::{
    constants::{EVENT_SEED, TICKET_SEED},
    error::ErrorCode,
    state::{EventConfig, TicketState},
    utils::{burn_ticket, load_ticket_asset},
};

#[derive(Accounts)]
pub struct CloseTicket<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [
            EVENT_SEED,
            event_config.organizer.as_ref(),
            event_config.event_id.to_le_bytes().as_ref(),
        ],
        bump = event_config.bump,
    )]
    pub event_config: Account<'info, EventConfig>,

    /// CHECK: Event collection; address and owner are constrained below.
    #[account(
        mut,
        address = event_config.collection @ ErrorCode::InvalidCollection,
        owner = MPL_CORE_ID,
    )]
    pub collection: UncheckedAccount<'info>,

    /// CHECK: Ticket Core asset; owned by mpl-core, collection and owner checked in handler
    #[account(mut, owner = MPL_CORE_ID)]
    pub ticket_asset: UncheckedAccount<'info>,

    #[account(
        mut,
        close = user,
        seeds = [TICKET_SEED, ticket_asset.key().as_ref()],
        bump = ticket_state.bump,
        constraint = ticket_state.event == event_config.key() @ ErrorCode::InvalidEvent,
    )]
    pub ticket_state: Account<'info, TicketState>,

    /// CHECK: Address is constrained to the Metaplex Core program ID.
    #[account(address = MPL_CORE_ID)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

// Owner's choice after the event: burn an unused ticket to reclaim its rent, or keep it.
// The owner can't burn it directly because the collection freezes it, so the program burns it
// with the permanent burn delegate and the owner as payer (mpl-core refunds the asset rent to them).
pub fn handle_close_ticket(ctx: Context<CloseTicket>) -> Result<()> {
    let event_config = &ctx.accounts.event_config;
    let current_time = Clock::get()?.unix_timestamp;

    require!(
        current_time >= event_config.event_end,
        ErrorCode::EventNotEnded
    );

    let asset = load_ticket_asset(&ctx.accounts.ticket_asset, event_config)?;
    require_keys_eq!(
        asset.owner,
        ctx.accounts.user.key(),
        ErrorCode::NotTicketOwner
    );

    let event_id_bytes = event_config.event_id.to_le_bytes();
    let bump_seed = [event_config.bump];

    let signer_seeds: &[&[u8]] = &[
        EVENT_SEED,
        event_config.organizer.as_ref(),
        event_id_bytes.as_ref(),
        &bump_seed,
    ];

    // The owner is both payer and rent recipient, so the asset rent goes straight to them
    burn_ticket(
        &ctx.accounts.mpl_core_program.to_account_info(),
        &ctx.accounts.ticket_asset.to_account_info(),
        &ctx.accounts.collection.to_account_info(),
        &ctx.accounts.event_config.to_account_info(),
        &ctx.accounts.user.to_account_info(),
        &ctx.accounts.user.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        signer_seeds,
    )?;

    Ok(())
}
