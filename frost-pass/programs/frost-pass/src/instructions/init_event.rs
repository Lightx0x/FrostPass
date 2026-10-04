use anchor_lang::prelude::*;
use mpl_core::{
    instructions::CreateCollectionV1CpiBuilder,
    types::{
        PermanentBurnDelegate, PermanentTransferDelegate, Plugin, PluginAuthorityPair,
    },
    ID as MPL_CORE_ID,
};

use crate::{
    constants::{EVENT_SEED, MAX_MARKUP_BPS, MAX_ROYALTY_BPS, MAX_SCANNERS},
    error::ErrorCode,
    state::EventConfig,
};

#[derive(Accounts)]
#[instruction(event_id: u32)]
pub struct InitEvent<'info> {
    #[account(mut)]
    pub organizer: Signer<'info>,

    #[account(
        init,
        payer = organizer,
        space = 8 + EventConfig::INIT_SPACE,
        seeds = [EVENT_SEED, event_id.to_le_bytes().as_ref()],
        bump
    )]
    pub event_config: Account<'info, EventConfig>,

    #[account(mut)]
    pub collection: Signer<'info>,

    /// CHECK: Metaplex Core Program
    #[account(address = MPL_CORE_ID)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handle_init_event(
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
    usdc_mint: Pubkey,
) -> Result<()> {
    let clock = Clock::get()?;

    require!(event_end > clock.unix_timestamp, ErrorCode::EventEndInPast);
    require!(ticket_supply > 0, ErrorCode::InvalidSupplyAmount);
    require!(ticket_price > 0, ErrorCode::InvalidPrice);
    require!(markup_cap_bps <= MAX_MARKUP_BPS, ErrorCode::ExceedsMaxAllowedMarkup);
    require!(royalty_bps <= MAX_ROYALTY_BPS, ErrorCode::ExceedsMaxAllowedRoyalty);
    require!(scanners.len() <= MAX_SCANNERS, ErrorCode::TooManyScanners);

    let event_config_info = ctx.accounts.event_config.to_account_info();

    let plugins = vec![
        PluginAuthorityPair {
            plugin: Plugin::PermanentTransferDelegate(PermanentTransferDelegate {}),
            authority: None,
        },
        PluginAuthorityPair {
            plugin: Plugin::PermanentBurnDelegate(PermanentBurnDelegate {}),
            authority: None,
        },
    ];

    CreateCollectionV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .collection(&ctx.accounts.collection.to_account_info())
        .payer(&ctx.accounts.organizer.to_account_info())
        .update_authority(Some(&event_config_info))
        .name(name)
        .uri(uri)
        .plugins(plugins)
        .invoke()?;

    let event_config = &mut ctx.accounts.event_config;
    event_config.event_id = event_id;
    event_config.organizer = ctx.accounts.organizer.key();
    event_config.usdc_mint = usdc_mint;
    event_config.collection = ctx.accounts.collection.key();
    event_config.ticket_price = ticket_price;
    event_config.ticket_supply = ticket_supply;
    event_config.tickets_minted = 0;
    event_config.markup_cap_bps = markup_cap_bps;
    event_config.royalty_bps = royalty_bps;
    event_config.event_end = event_end;
    event_config.scanners = scanners;
    event_config.bump = ctx.bumps.event_config;

    Ok(())
}
