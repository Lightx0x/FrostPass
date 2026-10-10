//! `init_event`: test plan #1–#3.

mod common;

use anchor_lang::prelude::Pubkey;
use common::*;
use frost_pass::{
    constants::{
        MAX_EVENT_DURATION, MAX_MARKUP_BPS, MAX_NAME_LENGTH, MAX_ROYALTY_BPS, MAX_SCANNERS,
        MAX_URI_LENGTH, MIN_EVENT_DURATION,
    },
    error::ErrorCode,
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
use solana_signer::Signer;

/// #1: the event config is stored exactly as given, and the collection carries all three
/// permanent plugins under the `EventConfig` PDA's authority, with the freeze on.
#[test]
fn init_event_stores_config_and_creates_frozen_collection() {
    let mut ctx = TestContext::new();
    let params = InitEventParams::default();
    let event = ctx.create_event(params.clone());

    let config = ctx.event_config(&event);
    let (_, expected_bump) = Pubkey::find_program_address(
        &[
            frost_pass::constants::EVENT_SEED,
            ctx.organizer.pubkey().as_ref(),
            &params.event_id.to_le_bytes(),
        ],
        &frost_pass::ID,
    );
    assert_eq!(config.event_id, params.event_id);
    assert_eq!(config.organizer, ctx.organizer.pubkey());
    assert_eq!(config.collection, event.collection);
    assert_eq!(config.ticket_price, params.ticket_price);
    assert_eq!(config.ticket_supply, params.ticket_supply);
    assert_eq!(config.tickets_minted, 0);
    assert_eq!(config.markup_cap_bps, params.markup_cap_bps);
    assert_eq!(config.royalty_bps, params.royalty_bps);
    assert_eq!(config.sales_end, params.sales_end);
    assert_eq!(config.event_end, params.event_end);
    assert_eq!(config.scanners, params.scanners);
    assert!(!config.cancelled);
    assert_eq!(config.bump, expected_bump);

    let mut collection_account = ctx
        .svm
        .get_account(&event.collection)
        .expect("collection should exist");
    assert_eq!(collection_account.owner, MPL_CORE_ID);

    let collection = BaseCollectionV1::from_bytes(&collection_account.data).unwrap();
    assert_eq!(collection.name, params.name);
    assert_eq!(collection.uri, params.uri);
    assert_eq!(
        collection.update_authority, event.config,
        "the collection's update authority must be the EventConfig PDA"
    );

    let info = anchor_lang::prelude::AccountInfo::new(
        &event.collection,
        false,
        false,
        &mut collection_account.lamports,
        &mut collection_account.data,
        &collection_account.owner,
        false,
    );
    let (transfer_authority, _, _) = fetch_plugin::<BaseCollectionV1, PermanentTransferDelegate>(
        &info,
        PluginType::PermanentTransferDelegate,
    )
    .expect("PermanentTransferDelegate must exist");
    assert_eq!(transfer_authority, PluginAuthority::UpdateAuthority);

    let (burn_authority, _, _) = fetch_plugin::<BaseCollectionV1, PermanentBurnDelegate>(
        &info,
        PluginType::PermanentBurnDelegate,
    )
    .expect("PermanentBurnDelegate must exist");
    assert_eq!(burn_authority, PluginAuthority::UpdateAuthority);

    let (freeze_authority, freeze, _) = fetch_plugin::<BaseCollectionV1, PermanentFreezeDelegate>(
        &info,
        PluginType::PermanentFreezeDelegate,
    )
    .expect("PermanentFreezeDelegate must exist");
    assert_eq!(freeze_authority, PluginAuthority::UpdateAuthority);
    assert!(
        freeze.frozen,
        "every ticket in the collection must start frozen"
    );
}

/// #2: every limit passes exactly at its edge and fails one step past it.
#[test]
fn init_event_enforces_limits_at_their_edges() {
    let mut ctx = TestContext::new();
    let mut next_id = 0;
    let mut params = |edit: &dyn Fn(&mut InitEventParams)| {
        next_id += 1;
        let mut params = InitEventParams::with_id(next_id);
        edit(&mut params);
        params
    };

    // (case, accepted at the edge, rejected one past it, expected error)
    let cases: Vec<(&str, InitEventParams, InitEventParams, ErrorCode)> = vec![
        (
            "markup cap",
            params(&|p| p.markup_cap_bps = MAX_MARKUP_BPS),
            params(&|p| p.markup_cap_bps = MAX_MARKUP_BPS + 1),
            ErrorCode::ExceedsMaxAllowedMarkup,
        ),
        (
            "royalty",
            params(&|p| p.royalty_bps = MAX_ROYALTY_BPS),
            params(&|p| p.royalty_bps = MAX_ROYALTY_BPS + 1),
            ErrorCode::ExceedsMaxAllowedRoyalty,
        ),
        (
            "name length",
            params(&|p| p.name = "a".repeat(MAX_NAME_LENGTH)),
            params(&|p| p.name = "a".repeat(MAX_NAME_LENGTH + 1)),
            ErrorCode::InvalidNameLength,
        ),
        (
            "uri length",
            params(&|p| p.uri = "x".repeat(MAX_URI_LENGTH)),
            params(&|p| p.uri = "x".repeat(MAX_URI_LENGTH + 1)),
            ErrorCode::InvalidUriLength,
        ),
        (
            "scanner count",
            params(&|p| p.scanners = new_pubkeys(MAX_SCANNERS)),
            params(&|p| p.scanners = new_pubkeys(MAX_SCANNERS + 1)),
            ErrorCode::TooManyScanners,
        ),
        (
            "minimum duration",
            params(&|p| {
                p.sales_end = MIN_EVENT_DURATION;
                p.event_end = MIN_EVENT_DURATION;
            }),
            params(&|p| {
                p.sales_end = MIN_EVENT_DURATION - 1;
                p.event_end = MIN_EVENT_DURATION - 1;
            }),
            ErrorCode::EventDurationTooShort,
        ),
        (
            "maximum duration",
            params(&|p| {
                p.sales_end = MAX_EVENT_DURATION;
                p.event_end = MAX_EVENT_DURATION;
            }),
            params(&|p| {
                p.sales_end = MAX_EVENT_DURATION;
                p.event_end = MAX_EVENT_DURATION + 1;
            }),
            ErrorCode::EventDurationTooLong,
        ),
    ];

    for (case, at_edge, past_edge, error) in cases {
        if let Err(logs) = ctx.try_create_event(at_edge) {
            panic!("{case}: the edge value should be accepted, logs:\n{logs}");
        }
        let logs = expect_failure(ctx.try_create_event(past_edge));
        assert_custom_error(&logs, error);
    }
}

/// #3: invalid inputs are rejected with their specific errors, and an event ID can't be reused.
#[test]
fn init_event_rejects_invalid_input() {
    let mut ctx = TestContext::new();
    let duplicate = Keypair::new().pubkey();

    let cases: Vec<(InitEventParams, ErrorCode)> = vec![
        (
            InitEventParams {
                ticket_price: 0,
                ..InitEventParams::with_id(1)
            },
            ErrorCode::InvalidPrice,
        ),
        (
            InitEventParams {
                ticket_supply: 0,
                ..InitEventParams::with_id(2)
            },
            ErrorCode::InvalidSupplyAmount,
        ),
        (
            InitEventParams {
                name: String::new(),
                ..InitEventParams::with_id(3)
            },
            ErrorCode::InvalidNameLength,
        ),
        (
            InitEventParams {
                uri: String::new(),
                ..InitEventParams::with_id(4)
            },
            ErrorCode::InvalidUriLength,
        ),
        (
            // sales_end is not in the future (the clock is at 0)
            InitEventParams {
                sales_end: 0,
                ..InitEventParams::with_id(5)
            },
            ErrorCode::InvalidSalesEnd,
        ),
        (
            InitEventParams {
                sales_end: EVENT_END + 1,
                ..InitEventParams::with_id(6)
            },
            ErrorCode::InvalidSalesEnd,
        ),
        (
            InitEventParams {
                scanners: vec![],
                ..InitEventParams::with_id(7)
            },
            ErrorCode::NoScannersProvided,
        ),
        (
            InitEventParams {
                scanners: vec![Pubkey::default()],
                ..InitEventParams::with_id(8)
            },
            ErrorCode::InvalidScanner,
        ),
        (
            InitEventParams {
                scanners: vec![duplicate, duplicate],
                ..InitEventParams::with_id(9)
            },
            ErrorCode::DuplicateScanner,
        ),
    ];

    for (params, error) in cases {
        let logs = expect_failure(ctx.try_create_event(params));
        assert_custom_error(&logs, error);
    }

    // The organizer's own key can't be used as the collection.
    let organizer = ctx.organizer.insecure_clone();
    let logs = expect_failure(
        ctx.try_create_event_with_collection(InitEventParams::with_id(10), &organizer),
    );
    assert_custom_error(&logs, ErrorCode::InvalidCollection);

    // Re-initializing an existing event ID fails because the EventConfig PDA already exists.
    ctx.create_event(InitEventParams::with_id(11));
    let logs = expect_failure(ctx.try_create_event(InitEventParams::with_id(11)));
    assert!(
        logs.contains("already in use"),
        "expected the EventConfig PDA to be in use, logs:\n{logs}"
    );
}

fn new_pubkeys(count: usize) -> Vec<Pubkey> {
    (0..count).map(|_| Keypair::new().pubkey()).collect()
}
