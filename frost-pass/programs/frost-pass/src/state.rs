use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct EventConfig {
    pub event_id: u32,
    pub organizer: Pubkey,
    pub usdc_mint: Pubkey,

    pub ticket_price: u64,
    pub ticket_supply: u32,
    pub tickets_minted: u32,

    pub markup_cap_bps: u16, // organizer's choice, <= MAX_MARKUP_BPS
    pub royalty_bps: u16,    // organizer's choice, <= MAX_ROYALTY_BPS

    pub event_end: i64,

    #[max_len(5)]
    pub scanners: Vec<Pubkey>,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct TicketState {
    pub event: Pubkey,
    pub resale_count: u8,
    pub seller: Option<Pubkey>, // Some(seller) = listed by this wallet, None = not for sale
    pub list_price: u64,
    pub bump: u8,
}
