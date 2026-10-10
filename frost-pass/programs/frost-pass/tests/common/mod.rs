//! Shared LiteSVM harness for the FrostPass integration tests.
//!
//! Every test runs against the real SBF build of the program (`target/deploy/frost_pass.so`) and
//! the official Metaplex Core binary (`tests/fixtures/mpl_core_program.so`). Run
//! `cargo build-sbf --manifest-path programs/frost-pass/Cargo.toml --sbf-out-dir target/deploy`
//! before `cargo test -p frost-pass`.
//!
//! USDC is a fake 6-decimal mint written directly at `USDC_MINT`, and token accounts are written
//! directly at their ATA addresses, so tests never depend on devnet state.

// Each integration test file compiles this module separately and uses a different subset of it.
#![allow(dead_code)]

use anchor_lang::{
    prelude::{Clock, Pubkey},
    solana_program::{instruction::Instruction, program_option::COption, program_pack::Pack},
    AccountDeserialize,
};
use anchor_spl::token::{
    spl_token::state::{Account as SplTokenAccount, AccountState, Mint as SplMint},
    ID as TOKEN_PROGRAM_ID,
};
use frost_pass::{
    constants::{CHALLENGE_NONCE_LENGTH, PROTOCOL_TREASURY, USDC_MINT},
    error::ErrorCode,
    state::{EventConfig, MinterRecord, TicketState},
    ID as FROST_PASS_ID,
};
pub use frost_pass_client::*;
use litesvm::{types::TransactionMetadata, LiteSVM};
use mpl_core::{
    accounts::{BaseAssetV1, BaseCollectionV1},
    fetch_plugin,
    instructions::TransferV1Builder,
    types::{Key, PermanentFreezeDelegate, PluginType},
    ID as MPL_CORE_ID,
};
use solana_account::Account;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;

pub const USDC_DECIMALS: u8 = 6;
pub const ONE_USDC: u64 = 1_000_000;
pub const SOL: u64 = 1_000_000_000;

/// USDC given to every funded wallet: enough for several mints and resales.
pub const WALLET_USDC: u64 = 1_000 * ONE_USDC;

/// Default event: 10 USDC tickets, 10% markup cap, 5% royalty, sales close after
/// one day and the event ends after two (the test clock starts at 0).
pub const TICKET_PRICE: u64 = 10 * ONE_USDC;
pub const MARKUP_CAP_BPS: u16 = 1_000;
pub const ROYALTY_BPS: u16 = 500;
pub const SALES_END: i64 = 24 * 3_600;
pub const EVENT_END: i64 = 48 * 3_600;
pub const EVENT_NAME: &str = "Coldplay Devnet Tour";
pub const EVENT_URI: &str = "https://arweave.net/coldplay-metadata.json";

// ---------------------------------------------------------------------------
// Context and transactions
// ---------------------------------------------------------------------------

pub struct TestContext {
    pub svm: LiteSVM,
    pub organizer: Keypair,
}

impl TestContext {
    /// Loads both programs, installs the fake USDC mint, and creates a funded organizer plus the
    /// treasury's USDC account (both receive mint payments).
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();

        let mpl_core_bytes = std::fs::read("tests/fixtures/mpl_core_program.so")
            .or_else(|_| std::fs::read("programs/frost-pass/tests/fixtures/mpl_core_program.so"))
            .expect(
                "Failed to read mpl_core_program.so fixture at tests/fixtures/mpl_core_program.so",
            );
        svm.add_program(MPL_CORE_ID, &mpl_core_bytes)
            .expect("Failed to load Metaplex Core program into LiteSVM");

        let frost_pass_bytes = std::fs::read("../../target/deploy/frost_pass.so")
            .or_else(|_| std::fs::read("target/deploy/frost_pass.so"))
            .expect(
                "Failed to read frost_pass.so binary. Ensure you run 'cargo build-sbf' or 'anchor build' before running tests",
            );
        svm.add_program(FROST_PASS_ID, &frost_pass_bytes)
            .expect("Failed to load FrostPass program into LiteSVM");

        let mut context = Self {
            svm,
            organizer: Keypair::new(),
        };
        context.install_usdc_mint();
        let organizer = context.organizer.pubkey();
        context.airdrop(&organizer, 10 * SOL);
        context.set_usdc_balance(&organizer, 0);
        context.set_usdc_balance(&PROTOCOL_TREASURY, 0);
        context
    }

    /// Sends a transaction signed by `signers` (the first one pays fees).
    ///
    /// The blockhash is expired first so that identical retries (e.g. the same instruction
    /// failing twice) aren't rejected as duplicates. On failure the program logs are returned.
    pub fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<TransactionMetadata, String> {
        self.svm.expire_blockhash();
        let message = Message::new(instructions, Some(&signers[0].pubkey()));
        let tx = Transaction::new(signers, message, self.svm.latest_blockhash());
        self.svm
            .send_transaction(tx)
            .map_err(|failed| failed.meta.logs.join("\n"))
    }

    pub fn airdrop(&mut self, wallet: &Pubkey, lamports: u64) {
        self.svm
            .airdrop(wallet, lamports)
            .expect("airdrop should succeed");
    }

    /// A new wallet with SOL for fees and rent, and `WALLET_USDC` in its USDC account.
    pub fn funded_wallet(&mut self) -> Keypair {
        let wallet = Keypair::new();
        self.airdrop(&wallet.pubkey(), 10 * SOL);
        self.set_usdc_balance(&wallet.pubkey(), WALLET_USDC);
        wallet
    }

    pub fn set_time(&mut self, unix_timestamp: i64) {
        let mut clock = self.svm.get_sysvar::<Clock>();
        clock.unix_timestamp = unix_timestamp;
        self.svm.set_sysvar(&clock);
    }

    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    // ---- fake USDC -------------------------------------------------------

    fn install_usdc_mint(&mut self) {
        let mint = SplMint {
            mint_authority: COption::None,
            supply: u64::MAX / 2,
            decimals: USDC_DECIMALS,
            is_initialized: true,
            freeze_authority: COption::None,
        };
        let mut data = vec![0u8; SplMint::LEN];
        SplMint::pack(mint, &mut data).unwrap();
        self.write_token_program_account(USDC_MINT, data);
    }

    /// Writes (or overwrites) `owner`'s USDC ATA with the given balance.
    pub fn set_usdc_balance(&mut self, owner: &Pubkey, amount: u64) {
        let token_account = SplTokenAccount {
            mint: USDC_MINT,
            owner: *owner,
            amount,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };
        let mut data = vec![0u8; SplTokenAccount::LEN];
        SplTokenAccount::pack(token_account, &mut data).unwrap();
        self.write_token_program_account(usdc_ata(owner), data);
    }

    fn write_token_program_account(&mut self, address: Pubkey, data: Vec<u8>) {
        let account = Account {
            lamports: self.svm.minimum_balance_for_rent_exemption(data.len()),
            data,
            owner: TOKEN_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        };
        self.svm.set_account(address, account).unwrap();
    }

    // ---- readers ---------------------------------------------------------

    pub fn usdc_balance(&self, owner: &Pubkey) -> u64 {
        let account = self
            .svm
            .get_account(&usdc_ata(owner))
            .expect("USDC account should exist");
        SplTokenAccount::unpack(&account.data).unwrap().amount
    }

    pub fn lamports(&self, address: &Pubkey) -> u64 {
        self.svm
            .get_account(address)
            .map_or(0, |account| account.lamports)
    }

    pub fn event_config(&self, event: &Event) -> EventConfig {
        self.anchor_account(&event.config)
            .expect("EventConfig should exist")
    }

    pub fn ticket_state(&self, ticket_asset: &Pubkey) -> Option<TicketState> {
        self.anchor_account(&ticket_state_pda(ticket_asset))
    }

    pub fn minter_record(&self, event: &Event, minter: &Pubkey) -> Option<MinterRecord> {
        self.anchor_account(&minter_record_pda(&event.config, minter))
    }

    fn anchor_account<T: AccountDeserialize>(&self, address: &Pubkey) -> Option<T> {
        let account = self.svm.get_account(address)?;
        if account.data.is_empty() {
            return None;
        }
        T::try_deserialize(&mut account.data.as_slice()).ok()
    }

    /// The live Core asset, or `None` once it has been burned (a burned asset is left as a
    /// tombstone that no longer deserializes as `AssetV1`).
    pub fn ticket_asset(&self, ticket_asset: &Pubkey) -> Option<BaseAssetV1> {
        let account = self.svm.get_account(ticket_asset)?;
        BaseAssetV1::from_bytes(&account.data)
            .ok()
            .filter(|asset| asset.key == Key::AssetV1)
    }

    pub fn collection(&self, event: &Event) -> BaseCollectionV1 {
        let account = self
            .svm
            .get_account(&event.collection)
            .expect("collection should exist");
        BaseCollectionV1::from_bytes(&account.data).expect("collection should deserialize")
    }

    /// Whether the event collection's `PermanentFreezeDelegate` is set to frozen.
    pub fn collection_is_frozen(&self, event: &Event) -> bool {
        let mut account = self
            .svm
            .get_account(&event.collection)
            .expect("collection should exist");
        let info = anchor_lang::prelude::AccountInfo::new(
            &event.collection,
            false,
            false,
            &mut account.lamports,
            &mut account.data,
            &account.owner,
            false,
        );
        let (_, freeze, _) = fetch_plugin::<BaseCollectionV1, PermanentFreezeDelegate>(
            &info,
            PluginType::PermanentFreezeDelegate,
        )
        .expect("collection should have a PermanentFreezeDelegate");
        freeze.frozen
    }

    // ---- high-level flows ----------------------------------------------

    pub fn try_create_event(&mut self, params: InitEventParams) -> Result<Event, String> {
        let collection = Keypair::new();
        self.try_create_event_with_collection(params, &collection)
    }

    pub fn try_create_event_with_collection(
        &mut self,
        params: InitEventParams,
        collection: &Keypair,
    ) -> Result<Event, String> {
        let organizer = self.organizer.insecure_clone();
        let event = Event::new(&organizer.pubkey(), params.event_id, &collection.pubkey());
        let ix = init_event_ix(&organizer.pubkey(), &event, params.into());
        self.send(&[ix], &[&organizer, collection])?;
        Ok(event)
    }

    pub fn create_event(&mut self, params: InitEventParams) -> Event {
        self.try_create_event(params)
            .unwrap_or_else(|logs| panic!("init_event failed:\n{logs}"))
    }

    pub fn try_mint_ticket(&mut self, event: &Event, minter: &Keypair) -> Result<Pubkey, String> {
        let ticket_asset = Keypair::new();
        let ix = mint_ticket_ix(event, &minter.pubkey(), &ticket_asset.pubkey());
        self.send(&[ix], &[minter, &ticket_asset])?;
        Ok(ticket_asset.pubkey())
    }

    pub fn mint_ticket(&mut self, event: &Event, minter: &Keypair) -> Pubkey {
        self.try_mint_ticket(event, minter)
            .unwrap_or_else(|logs| panic!("mint_ticket failed:\n{logs}"))
    }

    pub fn try_list_ticket(
        &mut self,
        event: &Event,
        seller: &Keypair,
        ticket_asset: &Pubkey,
        list_price: u64,
    ) -> Result<TransactionMetadata, String> {
        let ix = list_ticket_ix(event, &seller.pubkey(), ticket_asset, list_price);
        self.send(&[ix], &[seller])
    }

    pub fn list_ticket(
        &mut self,
        event: &Event,
        seller: &Keypair,
        ticket_asset: &Pubkey,
        list_price: u64,
    ) {
        self.try_list_ticket(event, seller, ticket_asset, list_price)
            .unwrap_or_else(|logs| panic!("list_ticket failed:\n{logs}"));
    }

    pub fn try_buy_ticket(
        &mut self,
        event: &Event,
        buyer: &Keypair,
        seller: &Pubkey,
        ticket_asset: &Pubkey,
        max_price: u64,
    ) -> Result<TransactionMetadata, String> {
        let ix = buy_ticket_ix(event, &buyer.pubkey(), seller, ticket_asset, max_price);
        self.send(&[ix], &[buyer])
    }

    /// Redeems with a challenge signed by `challenge_signer` over `challenge`.
    /// The happy path signs with the ticket owner over the matching challenge; rejection tests
    /// vary the scanner, the signer or the challenge.
    pub fn try_redeem_ticket(
        &mut self,
        event: &Event,
        scanner: &Keypair,
        user: &Pubkey,
        ticket_asset: &Pubkey,
        challenge: &RedeemChallenge,
        challenge_signer: &Keypair,
    ) -> Result<TransactionMetadata, String> {
        let ed25519_ix = challenge.signed_by(challenge_signer);
        let redeem_ix = redeem_ticket_ix(event, &scanner.pubkey(), user, ticket_asset, challenge);
        self.send(&[ed25519_ix, redeem_ix], &[scanner])
    }

    /// Redeems `ticket_asset` with a valid challenge signed by its owner.
    pub fn try_redeem_as_owner(
        &mut self,
        event: &Event,
        scanner: &Keypair,
        owner: &Keypair,
        ticket_asset: &Pubkey,
    ) -> Result<TransactionMetadata, String> {
        let challenge = challenge_for(event, ticket_asset, &owner.pubkey(), self.now());
        self.try_redeem_ticket(
            event,
            scanner,
            &owner.pubkey(),
            ticket_asset,
            &challenge,
            owner,
        )
    }

    pub fn try_update_scanners(
        &mut self,
        event: &Event,
        signer: &Keypair,
        scanners: Vec<Pubkey>,
    ) -> Result<TransactionMetadata, String> {
        let ix = update_scanners_ix(event, &signer.pubkey(), scanners);
        self.send(&[ix], &[signer])
    }

    pub fn try_cancel_event(
        &mut self,
        event: &Event,
        signer: &Keypair,
    ) -> Result<TransactionMetadata, String> {
        let ix = cancel_event_ix(event, &signer.pubkey());
        self.send(&[ix], &[signer])
    }

    pub fn try_refund_ticket(
        &mut self,
        event: &Event,
        signer: &Keypair,
        user: &Pubkey,
        ticket_asset: &Pubkey,
    ) -> Result<TransactionMetadata, String> {
        let ix = refund_ticket_ix(event, &signer.pubkey(), user, ticket_asset);
        self.send(&[ix], &[signer])
    }

    pub fn try_close_ticket(
        &mut self,
        event: &Event,
        user: &Keypair,
        ticket_asset: &Pubkey,
    ) -> Result<TransactionMetadata, String> {
        let ix = close_ticket_ix(event, &user.pubkey(), ticket_asset);
        self.send(&[ix], &[user])
    }

    /// The holder tries to move their ticket directly through Metaplex Core, bypassing FrostPass.
    pub fn try_direct_core_transfer(
        &mut self,
        event: &Event,
        holder: &Keypair,
        ticket_asset: &Pubkey,
        new_owner: &Pubkey,
    ) -> Result<TransactionMetadata, String> {
        let ix = TransferV1Builder::new()
            .asset(*ticket_asset)
            .collection(Some(event.collection))
            .payer(holder.pubkey())
            .authority(Some(holder.pubkey()))
            .new_owner(*new_owner)
            .instruction();
        self.send(&[ix], &[holder])
    }

    /// Overwrites the owner recorded in a Core asset account.
    ///
    /// Frozen tickets can't change hands outside the program, so a "stale listing" (the listed
    /// seller no longer holds the ticket) can't be produced through instructions. This edits the
    /// account directly to exercise the program's defensive owner check.
    pub fn force_asset_owner(&mut self, ticket_asset: &Pubkey, new_owner: &Pubkey) {
        let mut account = self
            .svm
            .get_account(ticket_asset)
            .expect("asset should exist");
        // BaseAssetV1 layout: key (1 byte) then owner (32 bytes).
        account.data[1..33].copy_from_slice(new_owner.as_ref());
        self.svm.set_account(*ticket_asset, account).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Event parameters and challenges
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct InitEventParams {
    pub event_id: u32,
    pub name: String,
    pub uri: String,
    pub ticket_price: u64,
    pub ticket_supply: u32,
    pub markup_cap_bps: u16,
    pub royalty_bps: u16,
    pub sales_end: i64,
    pub event_end: i64,
    pub scanners: Vec<Pubkey>,
}

impl Default for InitEventParams {
    fn default() -> Self {
        Self {
            event_id: 1,
            name: EVENT_NAME.to_string(),
            uri: EVENT_URI.to_string(),
            ticket_price: TICKET_PRICE,
            ticket_supply: 100,
            markup_cap_bps: MARKUP_CAP_BPS,
            royalty_bps: ROYALTY_BPS,
            sales_end: SALES_END,
            event_end: EVENT_END,
            scanners: vec![Keypair::new().pubkey(), Keypair::new().pubkey()],
        }
    }
}

impl InitEventParams {
    pub fn with_id(event_id: u32) -> Self {
        Self {
            event_id,
            ..Self::default()
        }
    }

    pub fn with_scanners(event_id: u32, scanners: &[&Keypair]) -> Self {
        Self {
            event_id,
            scanners: scanners.iter().map(|scanner| scanner.pubkey()).collect(),
            ..Self::default()
        }
    }
}

impl From<InitEventParams> for InitEventArgs {
    fn from(params: InitEventParams) -> Self {
        Self {
            event_id: params.event_id,
            name: params.name,
            uri: params.uri,
            ticket_price: params.ticket_price,
            ticket_supply: params.ticket_supply,
            markup_cap_bps: params.markup_cap_bps,
            royalty_bps: params.royalty_bps,
            sales_end: params.sales_end,
            event_end: params.event_end,
            scanners: params.scanners,
        }
    }
}

/// A redeem challenge with a fixed nonce, valid for one minute from `now`.
pub fn challenge_for(
    event: &Event,
    ticket_asset: &Pubkey,
    user: &Pubkey,
    now: i64,
) -> RedeemChallenge {
    RedeemChallenge {
        event_config: event.config,
        ticket_asset: *ticket_asset,
        user: *user,
        nonce: [7u8; CHALLENGE_NONCE_LENGTH],
        expiry: now + 60,
    }
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

/// Asserts the logs contain the given FrostPass error.
#[track_caller]
pub fn assert_custom_error(logs: &str, error: ErrorCode) {
    let code = error as u32 + 6000;
    let hex_code = format!("{code:#x}");
    let name = error.name();
    assert!(
        logs.contains(&hex_code) || logs.contains(&name),
        "Expected ErrorCode::{name} ({code}/{hex_code}), but logs were:\n{logs}"
    );
}

/// Asserts the logs contain the given built-in Anchor error (e.g. a failed `seeds` constraint).
#[track_caller]
pub fn assert_anchor_error(logs: &str, error: anchor_lang::error::ErrorCode) {
    let code = error as u32;
    let hex_code = format!("{code:#x}");
    let name = error.name();
    assert!(
        logs.contains(&hex_code) || logs.contains(&name),
        "Expected anchor ErrorCode::{name} ({code}/{hex_code}), but logs were:\n{logs}"
    );
}

/// Asserts that Metaplex Core rejected a transfer because of the collection's freeze: the owner's
/// authority is approved, but `PermanentFreezeDelegate` rejects the transfer.
#[track_caller]
pub fn assert_blocked_by_freeze(logs: &str) {
    assert!(
        logs.contains("permanent_freeze_delegate") && logs.contains("Reject"),
        "Expected the transfer to be rejected by PermanentFreezeDelegate, but logs were:\n{logs}"
    );
}

/// Unwraps a transaction that must fail and returns its logs.
#[track_caller]
pub fn expect_failure<T>(result: Result<T, String>) -> String {
    match result {
        Ok(_) => panic!("transaction was expected to fail but succeeded"),
        Err(logs) => logs,
    }
}

/// Unwraps a transaction that must succeed.
#[track_caller]
pub fn expect_success(result: Result<TransactionMetadata, String>) -> TransactionMetadata {
    result.unwrap_or_else(|logs| panic!("transaction was expected to succeed, logs:\n{logs}"))
}
