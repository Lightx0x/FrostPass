use anchor_lang::prelude::*;

use crate::{error::ErrorCode, state::TicketState};

#[derive(Accounts)]
pub struct DelistTicket<'info> {
    pub seller: Signer<'info>,

    // Only the listing changes, so the seller check is enough; no asset or event needed
    #[account(mut)]
    pub ticket_state: Account<'info, TicketState>,
}

pub fn handle_delist_ticket(ctx: Context<DelistTicket>) -> Result<()> {
    let ticket_state = &mut ctx.accounts.ticket_state;

    let listed_seller = ticket_state.seller.ok_or(ErrorCode::NotListed)?;
    require_keys_eq!(
        listed_seller,
        ctx.accounts.seller.key(),
        ErrorCode::NotSeller
    );

    ticket_state.seller = None;
    ticket_state.list_price = 0;

    Ok(())
}
