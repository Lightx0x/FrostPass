use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;
use mpl_core::{
    instructions::CreateCollectionV1CpiBuilder,
    types::{
        PermanentBurnDelegate, PermanentFreezeDelegate, PermanentTransferDelegate, Plugin,
        PluginAuthority, PluginAuthorityPair,
    },
    ID as MPL_CORE_ID,
};

use crate::{
    constants::{
        EVENT_SEED, MAX_EVENT_DURATION, MAX_MARKUP_BPS, MAX_NAME_LENGTH, MAX_ROYALTY_BPS,
        MAX_SCANNERS, MAX_URI_LENGTH, MIN_EVENT_DURATION, USDC_MINT,
    },
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
        space = EventConfig::DISCRIMINATOR.len() + EventConfig::INIT_SPACE,
        seeds = [EVENT_SEED, organizer.key().as_ref(), event_id.to_le_bytes().as_ref()],
        bump
    )]
    pub event_config: Account<'info, EventConfig>,

    #[account(mut)]
    pub collection: Signer<'info>,

    #[account(address = USDC_MINT)]
    pub usdc_mint: InterfaceAccount<'info, Mint>,

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
) -> Result<()> {
    let clock = Clock::get()?;

    // Time & Economic Validations
    require!(
        event_end >= clock.unix_timestamp + MIN_EVENT_DURATION,
        ErrorCode::EventDurationTooShort
    );
    require!(
        event_end <= clock.unix_timestamp + MAX_EVENT_DURATION,
        ErrorCode::EventDurationTooLong
    );
    require!(ticket_supply > 0, ErrorCode::InvalidSupplyAmount);
    require!(ticket_price > 0, ErrorCode::InvalidPrice);
    require!(markup_cap_bps <= MAX_MARKUP_BPS, ErrorCode::ExceedsMaxAllowedMarkup);
    require!(royalty_bps <= MAX_ROYALTY_BPS, ErrorCode::ExceedsMaxAllowedRoyalty);

    // Metadata Validations
    require!(!name.is_empty() && name.len() <= MAX_NAME_LENGTH, ErrorCode::InvalidNameLength);
    require!(!uri.is_empty() && uri.len() <= MAX_URI_LENGTH, ErrorCode::InvalidUriLength);

    // Scanner Validations
    require!(!scanners.is_empty(), ErrorCode::NoScannersProvided);
    require!(scanners.len() <= MAX_SCANNERS, ErrorCode::TooManyScanners);

    // Reject duplicate scanners
    for scanner in 0..scanners.len() {
        for next_scanner in (scanner + 1)..scanners.len() {
            require!(scanners[scanner] != scanners[next_scanner], ErrorCode::DuplicateScanner);
        }
    }

    let event_config_info = ctx.accounts.event_config.to_account_info();

    // Configure Permanent Plugins: Transfer, Burn, and Freeze (Anti-P2P bypass)
    let plugins = vec![
        PluginAuthorityPair {
            plugin: Plugin::PermanentTransferDelegate(PermanentTransferDelegate {}),
            authority: Some(PluginAuthority::UpdateAuthority),
        },
        PluginAuthorityPair {
            plugin: Plugin::PermanentBurnDelegate(PermanentBurnDelegate {}),
            authority: Some(PluginAuthority::UpdateAuthority),
        },
        PluginAuthorityPair {
            plugin: Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen: true }),
            authority: Some(PluginAuthority::UpdateAuthority),
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
    event_config.usdc_mint = ctx.accounts.usdc_mint.key();
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
