use anchor_lang::prelude::*;

use crate::{
    constants::EVENT_SEED, error::ErrorCode, state::EventConfig, utils::validate_scanners,
};

#[derive(Accounts)]
pub struct UpdateScanners<'info> {
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

// Replaces the event's scanner list, e.g. to revoke a lost or compromised scanner key.
// The account is already sized for MAX_SCANNERS entries, so no realloc is needed.
pub fn handle_update_scanners(ctx: Context<UpdateScanners>, scanners: Vec<Pubkey>) -> Result<()> {
    validate_scanners(&scanners)?;

    ctx.accounts.event_config.scanners = scanners;

    Ok(())
}
