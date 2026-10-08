use anchor_lang::prelude::*;
use mpl_core::ID as MPL_CORE_ID;

use crate::{
    constants::{BPS_DENOMINATOR, EVENT_SEED, MAX_RESALE, TICKET_SEED},
    error::ErrorCode,
    state::{EventConfig, TicketState},
    utils::load_ticket_asset,
};

#[derive(Accounts)]
pub struct ListTicket<'info> {
    pub seller: Signer<'info>,

    #[account(
        seeds = [
            EVENT_SEED,
            event_config.organizer.as_ref(),
            event_config.event_id.to_le_bytes().as_ref(),
        ],
        bump = event_config.bump,
    )]
    pub event_config: Account<'info, EventConfig>,

    /// CHECK: Ticket Core asset; owned by mpl-core, collection and owner checked in handler
    #[account(owner = MPL_CORE_ID)]
    pub ticket_asset: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [TICKET_SEED, ticket_asset.key().as_ref()],
        bump = ticket_state.bump,
        constraint = ticket_state.event == event_config.key() @ ErrorCode::InvalidEvent,
    )]
    pub ticket_state: Account<'info, TicketState>,
}

pub fn handle_list_ticket(ctx: Context<ListTicket>, list_price: u64) -> Result<()> {
    let event_config = &ctx.accounts.event_config;
    let current_time = Clock::get()?.unix_timestamp;

    require!(current_time < event_config.sales_end, ErrorCode::SalesEnded);

    let ticket_asset = load_ticket_asset(&ctx.accounts.ticket_asset, event_config)?;
    require!(
        ticket_asset.owner == ctx.accounts.seller.key(),
        ErrorCode::NotTicketOwner
    );

    let ticket_state = &ctx.accounts.ticket_state;
    require!(ticket_state.seller.is_none(), ErrorCode::AlreadyListed);
    require!(
        ticket_state.resale_count < MAX_RESALE,
        ErrorCode::ResaleLimitReached
    );

    // Price cap: ticket_price * (100% + markup) / 100%, measured from the original price
    let max_price = (event_config.ticket_price as u128)
        .checked_mul(BPS_DENOMINATOR as u128 + event_config.markup_cap_bps as u128)
        .ok_or(ErrorCode::MathOverflow)?
        .checked_div(BPS_DENOMINATOR as u128)
        .ok_or(ErrorCode::MathOverflow)?;

    require!(list_price as u128 <= max_price, ErrorCode::PriceAboveCap);

    let ticket_state = &mut ctx.accounts.ticket_state;
    ticket_state.seller = Some(ctx.accounts.seller.key());
    ticket_state.list_price = list_price;

    Ok(())
}
