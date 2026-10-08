use anchor_lang::prelude::*;

use crate::{constants::EVENT_SEED, error::ErrorCode, state::EventConfig};

#[derive(Accounts)]
pub struct CancelEvent<'info> {
    pub organizer: Signer<'info>,

    #[account(
        mut,
        has_one = organizer @ ErrorCode::NotOrganizer,
        seeds = [
            EVENT_SEED,
            organizer.key().as_ref(),
            event_config.event_id.to_le_bytes().as_ref(),
        ],
        bump = event_config.bump,
    )]
    pub event_config: Account<'info, EventConfig>,
}

// Marks the event cancelled: mint / list / buy / redeem stop, and the organizer can refund
// holders with refund_ticket (voluntary refunds, temporary until escrow).
pub fn handle_cancel_event(ctx: Context<CancelEvent>) -> Result<()> {
    let event_config = &mut ctx.accounts.event_config;

    require!(!event_config.cancelled, ErrorCode::EventCancelled);

    event_config.cancelled = true;

    Ok(())
}
