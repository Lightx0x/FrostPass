use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};
use mpl_core::{accounts::BaseCollectionV1, instructions::CreateV1CpiBuilder, ID as MPL_CORE_ID};

use crate::{
    constants::{EVENT_SEED, TICKET_SEED, USDC_MINT},
    error::ErrorCode,
    state::{EventConfig, TicketState},
};

#[derive(Accounts)]
pub struct MintTicket<'info> {
    #[account(mut)]
    pub minter: Signer<'info>,

    #[account(
        mut,
        seeds = [
            EVENT_SEED,
            event_config.organizer.as_ref(),
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

    #[account(mut)]
    pub ticket_asset: Signer<'info>,

    #[account(
        init,
        payer = minter,
        space = TicketState::DISCRIMINATOR.len() + TicketState::INIT_SPACE,
        seeds = [TICKET_SEED, ticket_asset.key().as_ref()],
        bump,
    )]
    pub ticket_state: Account<'info, TicketState>,

    #[account(address = USDC_MINT @ ErrorCode::InvalidUsdcMint)]
    pub usdc_mint: Account<'info, Mint>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = minter,
        associated_token::token_program = token_program,
    )]
    pub minter_usdc: Account<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = event_config.organizer,
        associated_token::token_program = token_program,
    )]
    pub organizer_usdc: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,

    /// CHECK: Address is constrained to the Metaplex Core program ID.
    #[account(address = MPL_CORE_ID)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handle_mint_ticket(ctx: Context<MintTicket>) -> Result<()> {
    let event_config = &ctx.accounts.event_config;
    let current_time = Clock::get()?.unix_timestamp;

    require!(current_time < event_config.event_end, ErrorCode::EventEnded);
    require!(
        event_config.tickets_minted < event_config.ticket_supply,
        ErrorCode::EventSoldOut
    );

    let collection_data = BaseCollectionV1::from_bytes(&ctx.accounts.collection.data.borrow())
        .map_err(|_| ErrorCode::InvalidCollection)?;

    let ticket_number = event_config
        .tickets_minted
        .checked_add(1)
        .ok_or(ErrorCode::MathOverflow)?;

    let name = format!("{} #{}", collection_data.name, ticket_number);
    let uri = collection_data.uri;

    token::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.minter_usdc.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: ctx.accounts.organizer_usdc.to_account_info(),
                authority: ctx.accounts.minter.to_account_info(),
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
        .asset(&ctx.accounts.ticket_asset.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .authority(Some(&ctx.accounts.event_config.to_account_info()))
        .payer(&ctx.accounts.minter.to_account_info())
        .owner(Some(&ctx.accounts.minter.to_account_info()))
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

    ctx.accounts.event_config.tickets_minted = ticket_number;

    Ok(())
}
