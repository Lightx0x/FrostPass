mod common;

use anchor_lang::{
    prelude::Pubkey, solana_program::instruction::Instruction, system_program, AccountDeserialize,
    InstructionData, ToAccountMetas,
};
use common::TestContext;
use frost_pass::{
    constants::{
        EVENT_SEED, MAX_EVENT_DURATION, MAX_MARKUP_BPS, MAX_NAME_LENGTH, MAX_ROYALTY_BPS,
        MAX_SCANNERS, MAX_URI_LENGTH, MIN_EVENT_DURATION,
    },
    error::ErrorCode,
    state::EventConfig,
    ID as FROST_PASS_ID,
};
use mpl_core::{
    accounts::BaseCollectionV1,
    fetch_plugin,
    types::{
        PermanentBurnDelegate, PermanentFreezeDelegate, PermanentTransferDelegate, PluginAuthority,
        PluginType,
    },
    ID as MPL_CORE_ID,
};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;

#[derive(Clone)]
struct InitEventParams {
    event_id: u32,
    name: String,
    uri: String,
    ticket_price: u64,
    ticket_supply: u32,
    markup_cap_bps: u16,
    royalty_bps: u16,
    sales_end: i64,
    event_end: i64,
    scanners: Vec<Pubkey>,
}

impl Default for InitEventParams {
    fn default() -> Self {
        Self {
            event_id: 1,
            name: "Coldplay Devnet Tour".to_string(),
            uri: "https://arweave.net/coldplay-metadata.json".to_string(),
            ticket_price: 10_000_000,
            ticket_supply: 100,
            markup_cap_bps: 1_000,
            royalty_bps: 500,
            sales_end: 3_600 * 24, // 1 day
            event_end: 3_600 * 48, // 2 days
            scanners: vec![Keypair::new().pubkey(), Keypair::new().pubkey()],
        }
    }
}

fn execute_init_event(
    context: &mut TestContext,
    collection: &Keypair,
    params: InitEventParams,
) -> Result<(), String> {
    let organizer_pubkey = context.organizer.pubkey();
    let (event_config_pda, _bump) = Pubkey::find_program_address(
        &[
            EVENT_SEED,
            organizer_pubkey.as_ref(),
            params.event_id.to_le_bytes().as_ref(),
        ],
        &FROST_PASS_ID,
    );

    let accounts = frost_pass::accounts::InitEvent {
        organizer: organizer_pubkey,
        event_config: event_config_pda,
        collection: collection.pubkey(),
        mpl_core_program: MPL_CORE_ID,
        system_program: system_program::ID,
    };

    let ix_data = frost_pass::instruction::InitEvent {
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
    };

    let instruction = Instruction {
        program_id: FROST_PASS_ID,
        accounts: accounts.to_account_metas(None),
        data: ix_data.data(),
    };

    let message = Message::new(&[instruction], Some(&organizer_pubkey));
    let blockhash = context.svm.latest_blockhash();
    let tx = Transaction::new(&[&context.organizer, collection], message, blockhash);

    let tx_res = context.svm.send_transaction(tx);
    match tx_res {
        Ok(_) => Ok(()),
        Err(err) => {
            let logs = err.meta.logs.join("\n");
            Err(logs)
        }
    }
}

#[track_caller]
fn assert_custom_error(logs: &str, error: ErrorCode) {
    let error_code_number = (error as u32) + 6000;
    let hex_code = format!("{:#x}", error_code_number);
    let name = error.name();
    let msg = error.to_string();

    assert!(
        logs.contains(&hex_code) || logs.contains(&name) || logs.contains(&msg),
        "Expected ErrorCode::{:?} ({}/{:#x}/{:?}), but logs were:\n{}",
        error,
        error_code_number,
        error_code_number,
        msg,
        logs
    );
}

// ---------------------------------------------------------------------------
// 1. SUCCESS & DEEP STATE / METAPLEX PLUGIN VALIDATION
// ---------------------------------------------------------------------------

#[test]
fn test_init_event_success_and_deep_verification() {
    let mut context = TestContext::new();
    let collection = Keypair::new();
    let params = InitEventParams::default();
    let event_id = params.event_id;
    let organizer_pubkey = context.organizer.pubkey();

    let (event_config_pda, bump) = Pubkey::find_program_address(
        &[
            EVENT_SEED,
            organizer_pubkey.as_ref(),
            event_id.to_le_bytes().as_ref(),
        ],
        &FROST_PASS_ID,
    );

    let res = execute_init_event(&mut context, &collection, params.clone());
    assert!(res.is_ok(), "Expected success, got: {:?}", res.err());

    // 1. Deep verify EventConfig deserialization & fields
    let event_config_account = context
        .svm
        .get_account(&event_config_pda)
        .expect("EventConfig account should exist");
    let mut data_slice: &[u8] = &event_config_account.data;
    let event_config =
        EventConfig::try_deserialize(&mut data_slice).expect("Failed to deserialize EventConfig");

    assert_eq!(event_config.event_id, event_id);
    assert_eq!(event_config.organizer, organizer_pubkey);
    assert_eq!(event_config.collection, collection.pubkey());
    assert_eq!(event_config.ticket_price, 10_000_000);
    assert_eq!(event_config.ticket_supply, 100);
    assert_eq!(event_config.tickets_minted, 0);
    assert_eq!(event_config.markup_cap_bps, 1_000);
    assert_eq!(event_config.royalty_bps, 500);
    assert_eq!(event_config.sales_end, 3_600 * 24);
    assert_eq!(event_config.event_end, 3_600 * 48);
    assert_eq!(event_config.scanners, params.scanners);
    assert!(!event_config.cancelled);
    assert_eq!(event_config.bump, bump);

    // 2. Deep verify Metaplex Core Collection account
    let mut collection_account = context
        .svm
        .get_account(&collection.pubkey())
        .expect("Collection account should exist");
    assert_eq!(collection_account.owner, MPL_CORE_ID);

    let base_collection = BaseCollectionV1::from_bytes(&collection_account.data)
        .expect("Failed to deserialize BaseCollectionV1");

    assert_eq!(base_collection.name, "Coldplay Devnet Tour");
    assert_eq!(
        base_collection.uri,
        "https://arweave.net/coldplay-metadata.json"
    );
    assert_eq!(
        base_collection.update_authority, event_config_pda,
        "Collection update_authority MUST be the EventConfig PDA"
    );

    // 3. Verify all 3 anti-P2P plugins installed on collection
    let collection_key = collection.pubkey();
    let account_info = anchor_lang::prelude::AccountInfo::new(
        &collection_key,
        false,
        false,
        &mut collection_account.lamports,
        &mut collection_account.data,
        &collection_account.owner,
        false,
    );

    let (transfer_auth, _, _) = fetch_plugin::<BaseCollectionV1, PermanentTransferDelegate>(
        &account_info,
        PluginType::PermanentTransferDelegate,
    )
    .expect("PermanentTransferDelegate must exist on collection");
    assert_eq!(transfer_auth, PluginAuthority::UpdateAuthority);

    let (burn_auth, _, _) = fetch_plugin::<BaseCollectionV1, PermanentBurnDelegate>(
        &account_info,
        PluginType::PermanentBurnDelegate,
    )
    .expect("PermanentBurnDelegate must exist on collection");
    assert_eq!(burn_auth, PluginAuthority::UpdateAuthority);

    let (freeze_auth, freeze_plugin, _) =
        fetch_plugin::<BaseCollectionV1, PermanentFreezeDelegate>(
            &account_info,
            PluginType::PermanentFreezeDelegate,
        )
        .expect("PermanentFreezeDelegate must exist on collection");
    assert_eq!(freeze_auth, PluginAuthority::UpdateAuthority);
    assert!(
        freeze_plugin.frozen,
        "Collection must be frozen = true to block unauthorized transfers"
    );
}

// ---------------------------------------------------------------------------
// 2. BOUNDARY TESTS (Exact values allowed vs +1 rejected)
// ---------------------------------------------------------------------------

#[test]
fn test_init_event_boundary_markup_cap() {
    let mut context = TestContext::new();

    // Exact maximum allowed (2000 bps) must succeed
    let mut params_max = InitEventParams::default();
    params_max.event_id = 101;
    params_max.markup_cap_bps = MAX_MARKUP_BPS;
    assert!(execute_init_event(&mut context, &Keypair::new(), params_max).is_ok());

    // 1 bps above maximum must fail
    let mut params_exceeded = InitEventParams::default();
    params_exceeded.event_id = 102;
    params_exceeded.markup_cap_bps = MAX_MARKUP_BPS + 1;
    let res = execute_init_event(&mut context, &Keypair::new(), params_exceeded);
    assert_custom_error(&res.unwrap_err(), ErrorCode::ExceedsMaxAllowedMarkup);
}

#[test]
fn test_init_event_boundary_royalty_bps() {
    let mut context = TestContext::new();

    // Exact maximum allowed (500 bps) must succeed
    let mut params_max = InitEventParams::default();
    params_max.event_id = 201;
    params_max.royalty_bps = MAX_ROYALTY_BPS;
    assert!(execute_init_event(&mut context, &Keypair::new(), params_max).is_ok());

    // 1 bps above maximum must fail
    let mut params_exceeded = InitEventParams::default();
    params_exceeded.event_id = 202;
    params_exceeded.royalty_bps = MAX_ROYALTY_BPS + 1;
    let res = execute_init_event(&mut context, &Keypair::new(), params_exceeded);
    assert_custom_error(&res.unwrap_err(), ErrorCode::ExceedsMaxAllowedRoyalty);
}

#[test]
fn test_init_event_boundary_name_length() {
    let mut context = TestContext::new();

    // Exact max name length (32 chars) must succeed
    let mut params_max = InitEventParams::default();
    params_max.event_id = 301;
    params_max.name = "a".repeat(MAX_NAME_LENGTH);
    assert!(execute_init_event(&mut context, &Keypair::new(), params_max).is_ok());

    // 33 chars must fail
    let mut params_exceeded = InitEventParams::default();
    params_exceeded.event_id = 302;
    params_exceeded.name = "a".repeat(MAX_NAME_LENGTH + 1);
    let res_exceeded = execute_init_event(&mut context, &Keypair::new(), params_exceeded);
    assert_custom_error(&res_exceeded.unwrap_err(), ErrorCode::InvalidNameLength);

    // Empty name must fail
    let mut params_empty = InitEventParams::default();
    params_empty.event_id = 303;
    params_empty.name = "".to_string();
    let res_empty = execute_init_event(&mut context, &Keypair::new(), params_empty);
    assert_custom_error(&res_empty.unwrap_err(), ErrorCode::InvalidNameLength);
}

#[test]
fn test_init_event_boundary_uri_length() {
    let mut context = TestContext::new();

    // Exact max uri length (200 chars) must succeed
    let mut params_max = InitEventParams::default();
    params_max.event_id = 401;
    params_max.uri = format!("https://arweave.net/{}", "x".repeat(MAX_URI_LENGTH - 20));
    assert!(execute_init_event(&mut context, &Keypair::new(), params_max).is_ok());

    // 201 chars must fail
    let mut params_exceeded = InitEventParams::default();
    params_exceeded.event_id = 402;
    params_exceeded.uri = "x".repeat(MAX_URI_LENGTH + 1);
    let res = execute_init_event(&mut context, &Keypair::new(), params_exceeded);
    assert_custom_error(&res.unwrap_err(), ErrorCode::InvalidUriLength);

    // Empty uri must fail
    let mut params_empty = InitEventParams::default();
    params_empty.event_id = 403;
    params_empty.uri = "".to_string();
    let res_empty = execute_init_event(&mut context, &Keypair::new(), params_empty);
    assert_custom_error(&res_empty.unwrap_err(), ErrorCode::InvalidUriLength);
}

#[test]
fn test_init_event_boundary_scanners_count() {
    let mut context = TestContext::new();

    // Exactly 5 scanners (MAX_SCANNERS) must succeed
    let mut params_max = InitEventParams::default();
    params_max.event_id = 501;
    params_max.scanners = (0..MAX_SCANNERS).map(|_| Keypair::new().pubkey()).collect();
    assert!(execute_init_event(&mut context, &Keypair::new(), params_max).is_ok());

    // 6 scanners must fail
    let mut params_exceeded = InitEventParams::default();
    params_exceeded.event_id = 502;
    params_exceeded.scanners = (0..MAX_SCANNERS + 1)
        .map(|_| Keypair::new().pubkey())
        .collect();
    let res = execute_init_event(&mut context, &Keypair::new(), params_exceeded);
    assert_custom_error(&res.unwrap_err(), ErrorCode::TooManyScanners);
}

#[test]
fn test_init_event_boundary_duration() {
    let mut context = TestContext::new();

    // Minimum duration: clock starts at 0 -> event_end = MIN_EVENT_DURATION (3600) must succeed
    let mut params_min = InitEventParams::default();
    params_min.event_id = 601;
    params_min.sales_end = MIN_EVENT_DURATION;
    params_min.event_end = MIN_EVENT_DURATION;
    assert!(execute_init_event(&mut context, &Keypair::new(), params_min).is_ok());

    // 1 second below minimum duration (3599) must fail
    let mut params_under = InitEventParams::default();
    params_under.event_id = 602;
    params_under.sales_end = MIN_EVENT_DURATION - 1;
    params_under.event_end = MIN_EVENT_DURATION - 1;
    let res_under = execute_init_event(&mut context, &Keypair::new(), params_under);
    assert_custom_error(&res_under.unwrap_err(), ErrorCode::EventDurationTooShort);

    // Maximum duration: event_end = MAX_EVENT_DURATION (365 days) must succeed
    let mut params_max = InitEventParams::default();
    params_max.event_id = 603;
    params_max.sales_end = MAX_EVENT_DURATION;
    params_max.event_end = MAX_EVENT_DURATION;
    assert!(execute_init_event(&mut context, &Keypair::new(), params_max).is_ok());

    // 1 second above maximum duration must fail
    let mut params_over = InitEventParams::default();
    params_over.event_id = 604;
    params_over.sales_end = MAX_EVENT_DURATION;
    params_over.event_end = MAX_EVENT_DURATION + 1;
    let res_over = execute_init_event(&mut context, &Keypair::new(), params_over);
    assert_custom_error(&res_over.unwrap_err(), ErrorCode::EventDurationTooLong);
}

// ---------------------------------------------------------------------------
// 3. SECURITY & ATTACK VECTORS
// ---------------------------------------------------------------------------

#[test]
fn test_init_event_fails_when_collection_equals_organizer() {
    let mut context = TestContext::new();
    let organizer_clone = context.organizer.insecure_clone();
    let params = InitEventParams::default();

    let res = execute_init_event(&mut context, &organizer_clone, params);
    assert_custom_error(&res.unwrap_err(), ErrorCode::InvalidCollection);
}

#[test]
fn test_init_event_fails_with_zero_price_or_supply() {
    let mut context = TestContext::new();

    let mut params_zero_price = InitEventParams::default();
    params_zero_price.event_id = 701;
    params_zero_price.ticket_price = 0;
    let res_price = execute_init_event(&mut context, &Keypair::new(), params_zero_price);
    assert_custom_error(&res_price.unwrap_err(), ErrorCode::InvalidPrice);

    let mut params_zero_supply = InitEventParams::default();
    params_zero_supply.event_id = 702;
    params_zero_supply.ticket_supply = 0;
    let res_supply = execute_init_event(&mut context, &Keypair::new(), params_zero_supply);
    assert_custom_error(&res_supply.unwrap_err(), ErrorCode::InvalidSupplyAmount);
}

#[test]
fn test_init_event_fails_with_invalid_sales_end() {
    let mut context = TestContext::new();

    // sales_end in the past (clock timestamp is 0)
    let mut params_past = InitEventParams::default();
    params_past.event_id = 801;
    params_past.sales_end = 0;
    let res_past = execute_init_event(&mut context, &Keypair::new(), params_past);
    assert_custom_error(&res_past.unwrap_err(), ErrorCode::InvalidSalesEnd);

    // sales_end > event_end
    let mut params_after = InitEventParams::default();
    params_after.event_id = 802;
    params_after.sales_end = 3_600 * 50;
    params_after.event_end = 3_600 * 48;
    let res_after = execute_init_event(&mut context, &Keypair::new(), params_after);
    assert_custom_error(&res_after.unwrap_err(), ErrorCode::InvalidSalesEnd);
}

#[test]
fn test_init_event_fails_with_invalid_scanners() {
    let mut context = TestContext::new();

    // Empty scanners
    let mut params_empty = InitEventParams::default();
    params_empty.event_id = 901;
    params_empty.scanners = vec![];
    let res_empty = execute_init_event(&mut context, &Keypair::new(), params_empty);
    assert_custom_error(&res_empty.unwrap_err(), ErrorCode::NoScannersProvided);

    // Duplicate scanners
    let dup_scanner = Keypair::new().pubkey();
    let mut params_dup = InitEventParams::default();
    params_dup.event_id = 902;
    params_dup.scanners = vec![dup_scanner, dup_scanner];
    let res_dup = execute_init_event(&mut context, &Keypair::new(), params_dup);
    assert_custom_error(&res_dup.unwrap_err(), ErrorCode::DuplicateScanner);

    // Default Pubkey scanner
    let mut params_default = InitEventParams::default();
    params_default.event_id = 903;
    params_default.scanners = vec![Pubkey::default()];
    let res_default = execute_init_event(&mut context, &Keypair::new(), params_default);
    assert_custom_error(&res_default.unwrap_err(), ErrorCode::InvalidScanner);
}

#[test]
fn test_init_event_fails_reinitializing_same_event_id() {
    let mut context = TestContext::new();
    let collection1 = Keypair::new();
    let collection2 = Keypair::new();

    let res1 = execute_init_event(&mut context, &collection1, InitEventParams::default());
    assert!(res1.is_ok());

    let res2 = execute_init_event(&mut context, &collection2, InitEventParams::default());
    assert!(
        res2.is_err(),
        "Expected failure when re-initializing same event_id"
    );
    let logs = res2.unwrap_err();
    assert!(
        logs.contains("already in use"),
        "Expected 'already in use' error in logs, got:\n{}",
        logs
    );
}
