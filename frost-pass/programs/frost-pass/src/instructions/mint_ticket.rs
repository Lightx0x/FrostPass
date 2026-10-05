use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};
use mpl_core::{instructions::CreateV1CpiBuilder, ID as MPL_CORE_ID};

use crate::{
    constants::{EVENT_SEED, MAX_NAME_LENGTH, MAX_URI_LENGTH, TICKET_SEED, USDC_MINT},
    error::ErrorCode,
    state::{EventConfig, TicketState},
};
#[derive(Accounts)]
pub struct MintTicket<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,

    #[account(
        mut,
        seeds = [
            EVENT_SEED,
            event_config.organizer.as_ref(),
            event_config.event_id.to_le_bytes().as_ref(),
        ],
        bump = event_config.bump,
    )]
    pub event_config: Account<'info, EventConfig>,

    #[account(
        mut,
        address = event_config.collection @ ErrorCode::InvalidEvent,
        owner = MPL_CORE_ID,
    )]
    pub collection: UncheckedAccount<'info>,

    #[account(mut)]
    pub ticket: Signer<'info>,

    #[account(
        init,
        payer = buyer,
        space = TicketState::DISCRIMINATOR.len() + TicketState::INIT_SPACE,
        seeds = [TICKET_SEED, ticket.key().as_ref()],
        bump,
    )]
    pub ticket_state: Account<'info, TicketState>,

    #[account(address = USDC_MINT @ ErrorCode::InvalidUsdcMint)]
    pub usdc_mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = buyer_usdc.owner == buyer.key() @ ErrorCode::InvalidOwner,
        constraint = buyer_usdc.mint == usdc_mint.key() @ ErrorCode::InvalidUsdcMint,
    )]
    pub buyer_usdc: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = organizer_usdc.owner == event_config.organizer @ ErrorCode::InvalidOwner,
        constraint = organizer_usdc.mint == usdc_mint.key() @ ErrorCode::InvalidUsdcMint,
    )]
    pub organizer_usdc: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,

    #[account(address = MPL_CORE_ID)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handle_mint_ticket(ctx: Context<MintTicket>, name: String, uri: String) -> Result<()> {
    let event_config = &ctx.accounts.event_config;
    let now = Clock::get()?.unix_timestamp;

    require!(now < event_config.event_end, ErrorCode::EventEnded);
    require!(
        event_config.tickets_minted < event_config.ticket_supply,
        ErrorCode::EventSoldOut
    );

    require!(
        !name.is_empty() && name.len() <= MAX_NAME_LENGTH,
        ErrorCode::InvalidNameLength
    );
    require!(
        !uri.is_empty() && uri.len() <= MAX_URI_LENGTH,
        ErrorCode::InvalidUriLength
    );

    token::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.buyer_usdc.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: ctx.accounts.organizer_usdc.to_account_info(),
                authority: ctx.accounts.buyer.to_account_info(),
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

    CreateV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .asset(&ctx.accounts.ticket.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .authority(Some(&ctx.accounts.event_config.to_account_info()))
        .payer(&ctx.accounts.buyer.to_account_info())
        .owner(Some(&ctx.accounts.buyer.to_account_info()))
        .name(name)
        .uri(uri)
        .system_program(&ctx.accounts.system_program.to_account_info())
        .invoke_signed(&[signer_seeds])?;

    let ticket_state = &mut ctx.accounts.ticket_state;
    ticket_state.event = ctx.accounts.event_config.key();
    ticket_state.resale_count = 0;
    ticket_state.seller = None;
    ticket_state.list_price = 0;
    ticket_state.bump = ctx.bumps.ticket_state;

    ctx.accounts.event_config.tickets_minted = ctx
        .accounts
        .event_config
        .tickets_minted
        .checked_add(1)
        .ok_or(ErrorCode::MathOverflow)?;

    Ok(())
}
