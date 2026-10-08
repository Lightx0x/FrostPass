use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};
use mpl_core::{instructions::TransferV1CpiBuilder, ID as MPL_CORE_ID};

use crate::{
    constants::{BPS_DENOMINATOR, EVENT_SEED, MAX_RESALE, TICKET_SEED, USDC_MINT},
    error::ErrorCode,
    state::{EventConfig, TicketState},
    utils::load_ticket_asset,
};

#[derive(Accounts)]
pub struct BuyTicket<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,

    /// CHECK: Verified against ticket_state.seller and ticket_asset.owner in handler
    pub seller: UncheckedAccount<'info>,

    #[account(
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

    /// CHECK: Ticket Core asset; owned by mpl-core, collection and owner checked in handler
    #[account(
        mut,
        owner = MPL_CORE_ID,
    )]
    pub ticket_asset: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [TICKET_SEED, ticket_asset.key().as_ref()],
        bump = ticket_state.bump,
        constraint = ticket_state.event == event_config.key() @ ErrorCode::InvalidEvent,
    )]
    pub ticket_state: Box<Account<'info, TicketState>>,

    #[account(address = USDC_MINT @ ErrorCode::InvalidUsdcMint)]
    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = buyer,
        associated_token::token_program = token_program,
    )]
    pub buyer_usdc: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = seller,
        associated_token::token_program = token_program,
    )]
    pub seller_usdc: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = event_config.organizer,
        associated_token::token_program = token_program,
    )]
    pub organizer_usdc: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,

    /// CHECK: Address is constrained to the Metaplex Core program ID.
    #[account(address = MPL_CORE_ID)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handle_buy_ticket(ctx: Context<BuyTicket>, max_price: u64) -> Result<()> {
    require_keys_neq!(
        ctx.accounts.buyer.key(),
        ctx.accounts.seller.key(),
        ErrorCode::BuyerIsSeller
    );

    let event_config = &ctx.accounts.event_config;
    let current_time = Clock::get()?.unix_timestamp;

    require!(!event_config.cancelled, ErrorCode::EventCancelled);
    require!(current_time < event_config.sales_end, ErrorCode::SalesEnded);

    let ticket_state = &ctx.accounts.ticket_state;
    let listed_seller = ticket_state.seller.ok_or(ErrorCode::NotListed)?;
    require_keys_eq!(
        listed_seller,
        ctx.accounts.seller.key(),
        ErrorCode::NotSeller
    );

    require!(
        ticket_state.resale_count < MAX_RESALE,
        ErrorCode::ResaleLimitReached
    );

    let ticket_asset = load_ticket_asset(&ctx.accounts.ticket_asset, event_config)?;
    require_keys_eq!(
        ticket_asset.owner,
        ctx.accounts.seller.key(),
        ErrorCode::NotTicketOwner
    );

    // max_price is the maximum listing price the buyer accepts (slippage protection).
    // The royalty is deducted directly from this amount, not added on top.
    let price = ticket_state.list_price;
    require!(price <= max_price, ErrorCode::PriceExceedsMax);

    // Resales pay no protocol fee: the price splits into the royalty and the seller's share
    let royalty = u64::try_from(
        (price as u128)
            .checked_mul(event_config.royalty_bps as u128)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(BPS_DENOMINATOR as u128)
            .ok_or(ErrorCode::MathOverflow)?,
    )
    .map_err(|_| ErrorCode::MathOverflow)?;

    let seller_amount = price.checked_sub(royalty).ok_or(ErrorCode::MathOverflow)?;

    // Transfer USDC to seller if nonzero (free listings are allowed)
    if seller_amount > 0 {
        token::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.buyer_usdc.to_account_info(),
                    mint: ctx.accounts.usdc_mint.to_account_info(),
                    to: ctx.accounts.seller_usdc.to_account_info(),
                    authority: ctx.accounts.buyer.to_account_info(),
                },
            ),
            seller_amount,
            ctx.accounts.usdc_mint.decimals,
        )?;
    }

    // Transfer USDC royalty to organizer if nonzero
    if royalty > 0 {
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
            royalty,
            ctx.accounts.usdc_mint.decimals,
        )?;
    }

    // Transfer Metaplex Core asset using event_config PDA as delegate authority
    let event_id_bytes = event_config.event_id.to_le_bytes();
    let bump_seed = [event_config.bump];

    let signer_seeds: &[&[u8]] = &[
        EVENT_SEED,
        event_config.organizer.as_ref(),
        event_id_bytes.as_ref(),
        &bump_seed,
    ];

    TransferV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .asset(&ctx.accounts.ticket_asset.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .payer(&ctx.accounts.buyer.to_account_info())
        .authority(Some(&ctx.accounts.event_config.to_account_info()))
        .new_owner(&ctx.accounts.buyer.to_account_info())
        .system_program(Some(&ctx.accounts.system_program.to_account_info()))
        .invoke_signed(&[signer_seeds])?;

    // Update ticket state: increment resale count and delist
    let ticket_state = &mut ctx.accounts.ticket_state;
    ticket_state.resale_count = ticket_state
        .resale_count
        .checked_add(1)
        .ok_or(ErrorCode::MathOverflow)?;
    ticket_state.seller = None;
    ticket_state.list_price = 0;

    Ok(())
}
