//! Off-chain helpers for calling the FrostPass program: account addresses, the redeem
//! challenge, and one builder per instruction.
//!
//! Shared by the LiteSVM tests and the CLI, so both build instructions the same way. Nothing here
//! sends transactions or depends on a particular RPC or test runtime.

use anchor_lang::{
    prelude::Pubkey, solana_program::instruction::Instruction, system_program, InstructionData,
    ToAccountMetas,
};
use anchor_spl::{associated_token::get_associated_token_address, token::ID as TOKEN_PROGRAM_ID};
use frost_pass::{
    constants::{
        CHALLENGE_DOMAIN, CHALLENGE_NONCE_LENGTH, EVENT_SEED, MINTER_SEED, PROTOCOL_TREASURY,
        TICKET_SEED, USDC_MINT,
    },
    ID as FROST_PASS_ID,
};
use mpl_core::ID as MPL_CORE_ID;
use solana_ed25519_program::new_ed25519_instruction_with_signature;
use solana_instructions_sysvar::ID as INSTRUCTIONS_SYSVAR_ID;
use solana_signer::Signer;

pub use frost_pass::instruction::InitEvent as InitEventArgs;

// ---------------------------------------------------------------------------
// Events and challenges
// ---------------------------------------------------------------------------

/// Addresses of an event.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub config: Pubkey,
    pub collection: Pubkey,
    pub organizer: Pubkey,
}

impl Event {
    pub fn new(organizer: &Pubkey, event_id: u32, collection: &Pubkey) -> Self {
        Self {
            config: event_config_pda(organizer, event_id),
            collection: *collection,
            organizer: *organizer,
        }
    }
}

/// Length of the redeem challenge message.
pub const CHALLENGE_MESSAGE_LENGTH: usize =
    CHALLENGE_DOMAIN.len() + 32 + 32 + 32 + CHALLENGE_NONCE_LENGTH + 8;

/// The 136-byte redeem challenge, built exactly as `redeem_ticket` rebuilds it on-chain:
/// `CHALLENGE_DOMAIN || event_config || ticket_asset || user || nonce || expiry (i64 LE)`.
#[derive(Clone, Debug)]
pub struct RedeemChallenge {
    pub event_config: Pubkey,
    pub ticket_asset: Pubkey,
    pub user: Pubkey,
    pub nonce: [u8; CHALLENGE_NONCE_LENGTH],
    pub expiry: i64,
}

impl RedeemChallenge {
    /// The bytes the ticket's owner signs.
    pub fn message(&self) -> Vec<u8> {
        let mut message = Vec::with_capacity(CHALLENGE_MESSAGE_LENGTH);
        message.extend_from_slice(CHALLENGE_DOMAIN);
        message.extend_from_slice(self.event_config.as_ref());
        message.extend_from_slice(self.ticket_asset.as_ref());
        message.extend_from_slice(self.user.as_ref());
        message.extend_from_slice(&self.nonce);
        message.extend_from_slice(&self.expiry.to_le_bytes());
        message
    }

    /// The Ed25519 precompile instruction for a signature made elsewhere (e.g. by the holder's
    /// wallet). It must go immediately before `redeem_ticket` in the transaction.
    pub fn ed25519_ix(&self, signer: &Pubkey, signature: &[u8; 64]) -> Instruction {
        new_ed25519_instruction_with_signature(&self.message(), signature, &signer.to_bytes())
    }

    /// The Ed25519 precompile instruction proving `signer` signed this challenge.
    pub fn signed_by(&self, signer: &impl Signer) -> Instruction {
        let signature: [u8; 64] = signer.sign_message(&self.message()).into();
        self.ed25519_ix(&signer.pubkey(), &signature)
    }
}

// ---------------------------------------------------------------------------
// Addresses
// ---------------------------------------------------------------------------

pub fn event_config_pda(organizer: &Pubkey, event_id: u32) -> Pubkey {
    Pubkey::find_program_address(
        &[EVENT_SEED, organizer.as_ref(), &event_id.to_le_bytes()],
        &FROST_PASS_ID,
    )
    .0
}

pub fn ticket_state_pda(ticket_asset: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[TICKET_SEED, ticket_asset.as_ref()], &FROST_PASS_ID).0
}

pub fn minter_record_pda(event_config: &Pubkey, minter: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[MINTER_SEED, event_config.as_ref(), minter.as_ref()],
        &FROST_PASS_ID,
    )
    .0
}

pub fn usdc_ata(owner: &Pubkey) -> Pubkey {
    get_associated_token_address(owner, &USDC_MINT)
}

// ---------------------------------------------------------------------------
// Instruction builders
// ---------------------------------------------------------------------------

fn frost_pass_ix(accounts: impl ToAccountMetas, data: impl InstructionData) -> Instruction {
    Instruction {
        program_id: FROST_PASS_ID,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

pub fn init_event_ix(organizer: &Pubkey, event: &Event, args: InitEventArgs) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::InitEvent {
            organizer: *organizer,
            event_config: event.config,
            collection: event.collection,
            mpl_core_program: MPL_CORE_ID,
            system_program: system_program::ID,
        },
        args,
    )
}

pub fn mint_ticket_ix(event: &Event, minter: &Pubkey, ticket_asset: &Pubkey) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::MintTicket {
            minter: *minter,
            event_config: event.config,
            collection: event.collection,
            ticket_asset: *ticket_asset,
            ticket_state: ticket_state_pda(ticket_asset),
            minter_record: minter_record_pda(&event.config, minter),
            usdc_mint: USDC_MINT,
            minter_usdc: usdc_ata(minter),
            organizer_usdc: usdc_ata(&event.organizer),
            treasury_usdc: usdc_ata(&PROTOCOL_TREASURY),
            token_program: TOKEN_PROGRAM_ID,
            mpl_core_program: MPL_CORE_ID,
            system_program: system_program::ID,
        },
        frost_pass::instruction::MintTicket {},
    )
}

pub fn list_ticket_ix(
    event: &Event,
    seller: &Pubkey,
    ticket_asset: &Pubkey,
    list_price: u64,
) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::ListTicket {
            seller: *seller,
            event_config: event.config,
            ticket_asset: *ticket_asset,
            ticket_state: ticket_state_pda(ticket_asset),
        },
        frost_pass::instruction::ListTicket { list_price },
    )
}

pub fn delist_ticket_ix(seller: &Pubkey, ticket_asset: &Pubkey) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::DelistTicket {
            seller: *seller,
            ticket_state: ticket_state_pda(ticket_asset),
        },
        frost_pass::instruction::DelistTicket {},
    )
}

pub fn buy_ticket_ix(
    event: &Event,
    buyer: &Pubkey,
    seller: &Pubkey,
    ticket_asset: &Pubkey,
    max_price: u64,
) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::BuyTicket {
            buyer: *buyer,
            seller: *seller,
            event_config: event.config,
            collection: event.collection,
            ticket_asset: *ticket_asset,
            ticket_state: ticket_state_pda(ticket_asset),
            usdc_mint: USDC_MINT,
            buyer_usdc: usdc_ata(buyer),
            seller_usdc: usdc_ata(seller),
            organizer_usdc: usdc_ata(&event.organizer),
            token_program: TOKEN_PROGRAM_ID,
            mpl_core_program: MPL_CORE_ID,
            system_program: system_program::ID,
        },
        frost_pass::instruction::BuyTicket { max_price },
    )
}

/// `redeem_ticket`. The transaction must put `challenge`'s Ed25519 instruction (from
/// `RedeemChallenge::signed_by` or `ed25519_ix`) immediately before this one.
pub fn redeem_ticket_ix(
    event: &Event,
    scanner: &Pubkey,
    user: &Pubkey,
    ticket_asset: &Pubkey,
    challenge: &RedeemChallenge,
) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::RedeemTicket {
            scanner: *scanner,
            user: *user,
            event_config: event.config,
            collection: event.collection,
            ticket_asset: *ticket_asset,
            ticket_state: ticket_state_pda(ticket_asset),
            instructions_sysvar: INSTRUCTIONS_SYSVAR_ID,
            mpl_core_program: MPL_CORE_ID,
            system_program: system_program::ID,
        },
        frost_pass::instruction::RedeemTicket {
            nonce: challenge.nonce,
            expiry: challenge.expiry,
        },
    )
}

pub fn update_scanners_ix(event: &Event, organizer: &Pubkey, scanners: Vec<Pubkey>) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::UpdateScanners {
            organizer: *organizer,
            event_config: event.config,
        },
        frost_pass::instruction::UpdateScanners { scanners },
    )
}

pub fn cancel_event_ix(event: &Event, organizer: &Pubkey) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::CancelEvent {
            organizer: *organizer,
            event_config: event.config,
        },
        frost_pass::instruction::CancelEvent {},
    )
}

pub fn refund_ticket_ix(
    event: &Event,
    organizer: &Pubkey,
    user: &Pubkey,
    ticket_asset: &Pubkey,
) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::RefundTicket {
            organizer: *organizer,
            user: *user,
            event_config: event.config,
            collection: event.collection,
            ticket_asset: *ticket_asset,
            ticket_state: ticket_state_pda(ticket_asset),
            usdc_mint: USDC_MINT,
            organizer_usdc: usdc_ata(organizer),
            user_usdc: usdc_ata(user),
            token_program: TOKEN_PROGRAM_ID,
            mpl_core_program: MPL_CORE_ID,
            system_program: system_program::ID,
        },
        frost_pass::instruction::RefundTicket {},
    )
}

pub fn close_ticket_ix(event: &Event, user: &Pubkey, ticket_asset: &Pubkey) -> Instruction {
    frost_pass_ix(
        frost_pass::accounts::CloseTicket {
            user: *user,
            event_config: event.config,
            collection: event.collection,
            ticket_asset: *ticket_asset,
            ticket_state: ticket_state_pda(ticket_asset),
            mpl_core_program: MPL_CORE_ID,
            system_program: system_program::ID,
        },
        frost_pass::instruction::CloseTicket {},
    )
}
