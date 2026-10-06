pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;
pub mod utils;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("8GqPBbxtkA9PbQYmAfb4SrU56tHiYaBh5kPUsHtM2hjk");

#[program]
pub mod frost_pass {
    use super::*;

    pub fn init_event(
        ctx: Context<InitEvent>,
        event_id: u32,
        name: String,
        uri: String,
        ticket_price: u64,
        ticket_supply: u32,
        markup_cap_bps: u16,
        royalty_bps: u16,
        event_end: i64,
        scanners: Vec<Pubkey>,
    ) -> Result<()> {
        instructions::handle_init_event(
            ctx,
            event_id,
            name,
            uri,
            ticket_price,
            ticket_supply,
            markup_cap_bps,
            royalty_bps,
            event_end,
            scanners,
        )
    }

    pub fn mint_ticket(ctx: Context<MintTicket>) -> Result<()> {
        instructions::handle_mint_ticket(ctx)
    }

    pub fn list_ticket(ctx: Context<ListTicket>, list_price: u64) -> Result<()> {
        instructions::handle_list_ticket(ctx, list_price)
    }

    pub fn delist_ticket(ctx: Context<DelistTicket>) -> Result<()> {
        instructions::handle_delist_ticket(ctx)
    }
}
