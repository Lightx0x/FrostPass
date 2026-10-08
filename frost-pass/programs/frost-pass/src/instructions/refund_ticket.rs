use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};
use mpl_core::ID as MPL_CORE_ID;

use crate::{
    constants::{EVENT_SEED, TICKET_SEED, USDC_MINT},
    error::ErrorCode,
    state::{EventConfig, TicketState},
    utils::{burn_ticket, load_ticket_asset},
};

#[derive(Accounts)]
pub struct RefundTicket<'info> {
    #[account(mut)]
    pub organizer: Signer<'info>,

    #[account(mut)]
    pub user: SystemAccount<'info>,

    #[account(
        has_one = organizer @ ErrorCode::NotOrganizer,
        seeds = [
            EVENT_SEED,
            organizer.key().as_ref(),
            event_config.event_id.to_le_bytes().as_ref(),
        ],
        bump = event_config.bump,
    )]
    pub event_config: Box<Account<'info, EventConfig>>,

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

    #[account(address = USDC_MINT @ ErrorCode::InvalidUsdcMint)]
    pub usdc_mint: Account<'info, Mint>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = organizer,
        associated_token::token_program = token_program,
    )]
    pub organizer_usdc: Account<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = user,
        associated_token::token_program = token_program,
    )]
    pub user_usdc: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,

    /// CHECK: Address is constrained to the Metaplex Core program ID.
    #[account(address = MPL_CORE_ID)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

// Voluntary refund on a cancelled event (temporary until escrow): the organizer pays the
// ticket's face value from their own wallet, then the ticket is burned and both rents go to `user`.
pub fn handle_refund_ticket(ctx: Context<RefundTicket>) -> Result<()> {
    let event_config = &ctx.accounts.event_config;

    // Only on a cancelled event, so the organizer can't force-buy tickets back at face value
    require!(event_config.cancelled, ErrorCode::EventNotCancelled);

    let asset = load_ticket_asset(&ctx.accounts.ticket_asset, event_config)?;
    require_keys_eq!(
        asset.owner,
        ctx.accounts.user.key(),
        ErrorCode::NotTicketOwner
    );

    // Refund face value to the current holder
    token::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.organizer_usdc.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: ctx.accounts.user_usdc.to_account_info(),
                authority: ctx.accounts.organizer.to_account_info(),
            },
        ),
        event_config.ticket_price,
        ctx.accounts.usdc_mint.decimals,
    )?;

    let event_id_bytes = event_config.event_id.to_le_bytes();
    let bump_seed = [event_config.bump];

    let signer_seeds: &[&[u8]] = &[
        EVENT_SEED,
        event_config.organizer.as_ref(),
        event_id_bytes.as_ref(),
        &bump_seed,
    ];

    // Burn with the organizer as payer and forward the asset rent to `user`, the current owner
    burn_ticket(
        &ctx.accounts.mpl_core_program.to_account_info(),
        &ctx.accounts.ticket_asset.to_account_info(),
        &ctx.accounts.collection.to_account_info(),
        &ctx.accounts.event_config.to_account_info(),
        &ctx.accounts.organizer.to_account_info(),
        &ctx.accounts.user.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        signer_seeds,
    )?;

    Ok(())
}
