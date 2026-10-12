//! Every actor's actions, end to end, against the real program in LiteSVM: the same `prepare_*`
//! → `execute` path the app uses, with the rule checks that stop an action before it's sent.
//!
//! Needs the program binary: `cargo build-sbf --manifest-path programs/frost-pass/Cargo.toml
//! --sbf-out-dir target/deploy` first (from `frost-pass/`).

use anchor_lang::{
    prelude::{Clock, Pubkey},
    solana_program::{instruction::Instruction, program_option::COption, program_pack::Pack},
};
use anchor_spl::token::{
    spl_token::state::{Account as SplTokenAccount, AccountState, Mint as SplMint},
    ID as TOKEN_PROGRAM_ID,
};
use frost_pass::{
    constants::{MINT_FEE, PROTOCOL_TREASURY, USDC_MINT},
    ID as FROST_PASS_ID,
};
use std::collections::HashMap;

use frost_pass_cli::{
    actions::{self, Plan},
    chain::{Chain, Filter, RawInstruction, SignatureInfo, TxDetails, TxError, TxRecord},
    checks::{NewEvent, Violation},
    format,
    history::{self, Fate, History},
    model,
    pass::Pass,
};
use frost_pass_client::usdc_ata;
use litesvm::LiteSVM;
use mpl_core::ID as MPL_CORE_ID;
use solana_account::Account;
use solana_keypair::Keypair;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::Transaction;

const USDC: u64 = 1_000_000;
const SOL: u64 = 1_000_000_000;
const START: i64 = 1_800_000_000;
const PRICE: u64 = 10 * USDC;

/// The largest serialized transaction a cluster accepts.
const PACKET_DATA_SIZE: usize = 1232;

struct SvmChain {
    svm: LiteSVM,
    /// Transactions sent so far, standing in for slots.
    sent: u64,
    /// The "slot" each account first appeared in, for [`Chain::first_slot`].
    first_seen: HashMap<Pubkey, u64>,
    details: HashMap<Signature, TxDetails>,
    /// Every landed transaction, oldest first, with the accounts it touched.
    landed: Vec<(TxRecord, Vec<Pubkey>)>,
}

impl Chain for SvmChain {
    fn account(&self, address: &Pubkey) -> anyhow::Result<Option<Account>> {
        Ok(self.svm.get_account(address).filter(|a| a.lamports > 0))
    }

    fn program_accounts(
        &self,
        program: &Pubkey,
        filters: &[Filter],
    ) -> anyhow::Result<Vec<(Pubkey, Account)>> {
        Ok(self
            .svm
            .accounts_db()
            .inner
            .iter()
            .map(|(address, account)| (*address, Account::from(account.clone())))
            .filter(|(_, account)| {
                account.owner == *program
                    && account.lamports > 0
                    && filters.iter().all(|filter| filter.matches(&account.data))
            })
            .collect())
    }

    fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Signature, TxError> {
        self.svm.expire_blockhash();
        let payer = signers[0].pubkey();
        let transaction = Transaction::new_signed_with_payer(
            instructions,
            Some(&payer),
            signers,
            self.svm.latest_blockhash(),
        );
        // A real cluster drops anything bigger; LiteSVM doesn't check
        let size = 1 + 64 * transaction.signatures.len() + transaction.message.serialize().len();
        assert!(size <= PACKET_DATA_SIZE, "transaction is {size} bytes");
        let keys = transaction.message.account_keys.clone();
        let compiled = transaction.message.instructions.clone();
        let (sol_before, usdc_before) = (self.account(&payer).unwrap(), self.usdc(&payer));
        let meta = self
            .svm
            .send_transaction(transaction)
            .map_err(|failed| TxError {
                message: format!("{:?}", failed.err),
                logs: failed.meta.logs,
            })?;
        self.sent += 1;
        let raw = |program: u8, accounts: &[u8], data: &[u8]| RawInstruction {
            program: keys[program as usize],
            accounts: accounts.iter().map(|&i| keys[i as usize]).collect(),
            data: data.to_vec(),
            inner: Vec::new(),
        };
        let instructions = compiled
            .iter()
            .enumerate()
            .map(|(index, ix)| RawInstruction {
                inner: meta
                    .inner_instructions
                    .get(index)
                    .into_iter()
                    .flatten()
                    .map(|inner| {
                        let ix = &inner.instruction;
                        raw(ix.program_id_index, &ix.accounts, &ix.data)
                    })
                    .collect(),
                ..raw(ix.program_id_index, &ix.accounts, &ix.data)
            })
            .collect();
        self.landed.push((
            TxRecord {
                signature: meta.signature,
                slot: self.sent,
                block_time: Some(self.now().unwrap()),
                instructions,
            },
            keys.clone(),
        ));
        for key in keys {
            if self.account(&key).unwrap().is_some() {
                self.first_seen.entry(key).or_insert(self.sent);
            }
        }
        let sol_after = self.account(&payer).unwrap().map_or(0, |a| a.lamports);
        self.details.insert(
            meta.signature,
            TxDetails {
                slot: self.sent,
                block_time: Some(self.now().unwrap()),
                fee: 5_000 * signers.len() as u64,
                compute_units: Some(meta.compute_units_consumed),
                sol_change: sol_after as i64 - sol_before.map_or(0, |a| a.lamports) as i64,
                usdc_change: self.usdc(&payer) as i64 - usdc_before as i64,
                logs: meta.logs,
            },
        );
        Ok(meta.signature)
    }

    fn transaction(
        &self,
        signature: &Signature,
        _wallet: &Pubkey,
    ) -> anyhow::Result<Option<TxDetails>> {
        Ok(self.details.get(signature).cloned())
    }

    fn first_slot(&self, address: &Pubkey) -> anyhow::Result<Option<u64>> {
        Ok(self.first_seen.get(address).copied())
    }

    fn signatures(
        &self,
        address: &Pubkey,
        until: Option<&Signature>,
    ) -> anyhow::Result<Vec<SignatureInfo>> {
        Ok(self
            .landed
            .iter()
            .rev()
            .take_while(|(record, _)| Some(&record.signature) != until)
            .filter(|(_, keys)| keys.contains(address))
            .map(|(record, _)| SignatureInfo {
                signature: record.signature,
                slot: record.slot,
                block_time: record.block_time,
                failed: false,
            })
            .collect())
    }

    fn record(&self, signature: &Signature) -> anyhow::Result<Option<TxRecord>> {
        Ok(self
            .landed
            .iter()
            .find(|(record, _)| record.signature == *signature)
            .map(|(record, _)| record.clone()))
    }
}

impl SvmChain {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let mpl_core = std::fs::read("../programs/frost-pass/tests/fixtures/mpl_core_program.so")
            .expect("mpl-core fixture");
        svm.add_program(MPL_CORE_ID, &mpl_core).unwrap();
        let frost_pass = std::fs::read("../target/deploy/frost_pass.so")
            .expect("run cargo build-sbf first (see the module docs)");
        svm.add_program(FROST_PASS_ID, &frost_pass).unwrap();

        let mut mint = vec![0u8; SplMint::LEN];
        SplMint::pack(
            SplMint {
                mint_authority: COption::None,
                supply: u64::MAX / 2,
                decimals: 6,
                is_initialized: true,
                freeze_authority: COption::None,
            },
            &mut mint,
        )
        .unwrap();
        let mut chain = Self {
            svm,
            sent: 0,
            first_seen: HashMap::new(),
            details: HashMap::new(),
            landed: Vec::new(),
        };
        chain.write_token_account(USDC_MINT, mint);
        chain.set_usdc(&PROTOCOL_TREASURY, 0);
        chain.set_time(START);
        chain
    }

    fn write_token_account(&mut self, address: Pubkey, data: Vec<u8>) {
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                address,
                Account {
                    lamports,
                    data,
                    owner: TOKEN_PROGRAM_ID,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    fn set_usdc(&mut self, owner: &Pubkey, amount: u64) {
        let mut data = vec![0u8; SplTokenAccount::LEN];
        SplTokenAccount::pack(
            SplTokenAccount {
                mint: USDC_MINT,
                owner: *owner,
                amount,
                delegate: COption::None,
                state: AccountState::Initialized,
                is_native: COption::None,
                delegated_amount: 0,
                close_authority: COption::None,
            },
            &mut data,
        )
        .unwrap();
        self.write_token_account(usdc_ata(owner), data);
    }

    fn usdc(&self, owner: &Pubkey) -> u64 {
        model::usdc_balance(self, owner).unwrap().unwrap_or(0)
    }

    fn set_time(&mut self, unix_timestamp: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp = unix_timestamp;
        self.svm.set_sysvar(&clock);
    }

    /// A wallet with SOL and, optionally, USDC.
    fn wallet(&mut self, usdc: Option<u64>) -> Keypair {
        let wallet = Keypair::new();
        self.svm.airdrop(&wallet.pubkey(), 10 * SOL).unwrap();
        if let Some(amount) = usdc {
            self.set_usdc(&wallet.pubkey(), amount);
        }
        wallet
    }

    fn run(&mut self, wallet: &Keypair, plan: anyhow::Result<Plan>) -> actions::Receipt {
        actions::execute(
            self,
            wallet,
            plan.expect("the action should pass its checks"),
        )
        .expect("the transaction should succeed")
    }
}

fn new_event(event_id: u32, scanners: Vec<Pubkey>) -> NewEvent {
    NewEvent {
        event_id,
        name: "Coldplay Devnet Tour".into(),
        uri: "https://arweave.net/coldplay.json".into(),
        ticket_price: PRICE,
        ticket_supply: 100,
        markup_cap_bps: 1_000,
        royalty_bps: 500,
        sales_end: START + 24 * 3_600,
        event_end: START + 48 * 3_600,
        scanners,
    }
}

/// The rule an action was blocked by, before anything was sent.
fn blocked<T>(result: anyhow::Result<T>) -> &'static str {
    match result {
        Ok(_) => panic!("expected the action to be blocked"),
        Err(error) => {
            error
                .downcast_ref::<Violation>()
                .unwrap_or_else(|| panic!("expected a rule violation, got: {error}"))
                .rule
        }
    }
}

/// Organizer → minter → seller → buyer → holder's pass → scanner, with the money and the ticket
/// checked at each step and the main ways each actor can break a rule blocked client-side.
#[test]
fn every_actor_completes_the_ticket_lifecycle() {
    let mut chain = SvmChain::new();
    let organizer = chain.wallet(None); // no USDC account yet: creating the event makes one
    let scanner = chain.wallet(None);
    let minter = chain.wallet(Some(100 * USDC));
    let buyer = chain.wallet(Some(100 * USDC));

    // Organizer
    let created = chain.run(
        &organizer,
        actions::prepare_init_event(&chain, &organizer, new_event(7, vec![scanner.pubkey()])),
    );
    let event_address = created.created.unwrap();
    assert_eq!(
        chain.usdc(&organizer.pubkey()),
        0,
        "the organizer's USDC account exists"
    );
    let events = model::events(&chain, Some(&organizer.pubkey())).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, "Coldplay Devnet Tour");

    // Minter: pays price + fee. The organizer can't mint their own tickets
    assert_eq!(
        blocked(actions::prepare_mint(&chain, &organizer, &event_address)),
        "ConstraintDuplicateMutableAccount"
    );
    let ticket = chain
        .run(
            &minter,
            actions::prepare_mint(&chain, &minter, &event_address),
        )
        .created
        .unwrap();
    assert_eq!(chain.usdc(&minter.pubkey()), 100 * USDC - PRICE - MINT_FEE);
    assert_eq!(chain.usdc(&organizer.pubkey()), PRICE);
    assert_eq!(chain.usdc(&PROTOCOL_TREASURY), MINT_FEE);
    let event = model::event(&chain, &event_address).unwrap();
    let tickets = model::event_tickets(&chain, &event).unwrap();
    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].owner, minter.pubkey());
    assert_eq!(tickets[0].name, "Coldplay Devnet Tour #1");

    // Seller: the cap is face value + 10%
    let cap = 11 * USDC;
    assert_eq!(
        blocked(actions::prepare_list(&chain, &minter, &ticket, cap + 1)),
        "PriceAboveCap"
    );
    assert_eq!(
        blocked(actions::prepare_list(&chain, &buyer, &ticket, cap)),
        "NotTicketOwner"
    );
    chain.run(
        &minter,
        actions::prepare_list(&chain, &minter, &ticket, cap),
    );

    // Buyer: not their own listing, and not above their maximum
    assert_eq!(
        blocked(actions::prepare_buy(&chain, &minter, &ticket, cap)),
        "BuyerIsSeller"
    );
    assert_eq!(
        blocked(actions::prepare_buy(&chain, &buyer, &ticket, cap - 1)),
        "PriceExceedsMax"
    );
    chain.run(&buyer, actions::prepare_buy(&chain, &buyer, &ticket, cap));
    let royalty = cap * 500 / 10_000;
    assert_eq!(chain.usdc(&buyer.pubkey()), 100 * USDC - cap);
    assert_eq!(
        chain.usdc(&minter.pubkey()),
        100 * USDC - PRICE - MINT_FEE + cap - royalty
    );
    assert_eq!(chain.usdc(&organizer.pubkey()), PRICE + royalty);
    assert_eq!(
        chain.usdc(&PROTOCOL_TREASURY),
        MINT_FEE,
        "no protocol fee on resales"
    );

    // Holder's pass: only the current owner can make one
    assert_eq!(
        blocked(actions::issue_pass(&chain, &minter, &ticket)),
        "NotTicketOwner"
    );
    let (pass, _, _) = actions::issue_pass(&chain, &buyer, &ticket).unwrap();
    let text = pass.encode();

    // Scanner: a stranger's check fails, the listed scanner's passes
    let stranger = chain.wallet(None);
    let report = actions::inspect_pass(&chain, &stranger.pubkey(), &text).unwrap();
    assert!(!report.admits());
    assert_eq!(
        blocked(actions::prepare_redeem(&chain, &stranger, &pass)),
        "InvalidScanner"
    );
    let report = actions::inspect_pass(&chain, &scanner.pubkey(), &text).unwrap();
    assert!(report.admits(), "every check should pass");

    // A tampered pass never reaches the chain
    let mut forged = Pass::decode(&text).unwrap();
    forged.expiry += 30;
    assert_eq!(
        blocked(actions::prepare_redeem(&chain, &scanner, &forged)),
        "InvalidChallenge"
    );

    let buyer_sol = model::sol_balance(&chain, &buyer.pubkey()).unwrap();
    chain.run(&scanner, actions::prepare_redeem(&chain, &scanner, &pass));
    assert!(
        model::ticket(&chain, &ticket).is_err(),
        "the ticket is burned"
    );
    assert!(
        model::sol_balance(&chain, &buyer.pubkey()).unwrap() > buyer_sol,
        "the holder gets both rents back"
    );
    assert!(model::event_tickets(&chain, &event).unwrap().is_empty());
}

/// Organizer rules: invalid settings and taken IDs are caught before sending, only the
/// organizer manages scanners, and a revoked scanner is turned away at the door.
#[test]
fn organizer_rules_and_scanner_revocation() {
    let mut chain = SvmChain::new();
    let organizer = chain.wallet(Some(0));
    let (old_scanner, new_scanner) = (chain.wallet(None), chain.wallet(None));
    let holder = chain.wallet(Some(100 * USDC));

    let too_much_royalty = NewEvent {
        royalty_bps: 600,
        ..new_event(1, vec![old_scanner.pubkey()])
    };
    assert_eq!(
        blocked(actions::prepare_init_event(
            &chain,
            &organizer,
            too_much_royalty
        )),
        "ExceedsMaxAllowedRoyalty"
    );
    let sales_after_event = NewEvent {
        sales_end: START + 49 * 3_600,
        ..new_event(1, vec![old_scanner.pubkey()])
    };
    assert_eq!(
        blocked(actions::prepare_init_event(
            &chain,
            &organizer,
            sales_after_event
        )),
        "InvalidSalesEnd"
    );
    let event = chain
        .run(
            &organizer,
            actions::prepare_init_event(
                &chain,
                &organizer,
                new_event(1, vec![old_scanner.pubkey()]),
            ),
        )
        .created
        .unwrap();
    assert_eq!(
        blocked(actions::prepare_init_event(
            &chain,
            &organizer,
            new_event(1, vec![old_scanner.pubkey()])
        )),
        "EventIdTaken"
    );

    // Only the organizer changes scanners, and the list is validated
    assert_eq!(
        blocked(actions::prepare_update_scanners(
            &chain,
            &holder,
            &event,
            vec![holder.pubkey()]
        )),
        "NotOrganizer"
    );
    assert_eq!(
        blocked(actions::prepare_update_scanners(
            &chain,
            &organizer,
            &event,
            vec![new_scanner.pubkey(), new_scanner.pubkey()]
        )),
        "DuplicateScanner"
    );
    let plan =
        actions::prepare_update_scanners(&chain, &organizer, &event, vec![new_scanner.pubkey()])
            .unwrap();
    let line = |label: &str| {
        plan.lines
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(line("Added"), Some(format::short(&new_scanner.pubkey())));
    assert_eq!(line("Revoked"), Some(format::short(&old_scanner.pubkey())));
    chain.run(&organizer, Ok(plan));

    let ticket = chain
        .run(&holder, actions::prepare_mint(&chain, &holder, &event))
        .created
        .unwrap();
    let (pass, _, _) = actions::issue_pass(&chain, &holder, &ticket).unwrap();
    assert_eq!(
        blocked(actions::prepare_redeem(&chain, &old_scanner, &pass)),
        "InvalidScanner"
    );
    chain.run(
        &new_scanner,
        actions::prepare_redeem(&chain, &new_scanner, &pass),
    );
}

/// Cancellation stops sales and enables face-value refunds; after the event ends, owners close
/// the tickets they kept.
#[test]
fn cancelled_event_refunds_and_ended_event_closes() {
    let mut chain = SvmChain::new();
    let organizer = chain.wallet(Some(100 * USDC));
    let scanner = chain.wallet(None);
    let holder = chain.wallet(Some(100 * USDC));
    let event = chain
        .run(
            &organizer,
            actions::prepare_init_event(&chain, &organizer, new_event(3, vec![scanner.pubkey()])),
        )
        .created
        .unwrap();
    let refunded = chain
        .run(&holder, actions::prepare_mint(&chain, &holder, &event))
        .created
        .unwrap();
    let kept = chain
        .run(&holder, actions::prepare_mint(&chain, &holder, &event))
        .created
        .unwrap();

    // Refunds need a cancelled event; only the organizer cancels
    assert_eq!(
        blocked(actions::prepare_refund(&chain, &organizer, &refunded)),
        "EventNotCancelled"
    );
    assert_eq!(
        blocked(actions::prepare_cancel(&chain, &holder, &event)),
        "NotOrganizer"
    );
    chain.run(
        &organizer,
        actions::prepare_cancel(&chain, &organizer, &event),
    );
    assert_eq!(
        blocked(actions::prepare_mint(&chain, &holder, &event)),
        "EventCancelled"
    );
    assert_eq!(
        blocked(actions::issue_pass(&chain, &holder, &kept)),
        "EventCancelled"
    );

    let holder_usdc = chain.usdc(&holder.pubkey());
    chain.run(
        &organizer,
        actions::prepare_refund(&chain, &organizer, &refunded),
    );
    assert_eq!(
        chain.usdc(&holder.pubkey()),
        holder_usdc + PRICE,
        "face value back"
    );
    assert!(
        model::ticket(&chain, &refunded).is_err(),
        "the refunded ticket is burned"
    );

    // Closing waits for the event's end
    assert_eq!(
        blocked(actions::prepare_close(&chain, &holder, &kept)),
        "EventNotEnded"
    );
    chain.set_time(START + 48 * 3_600);
    assert_eq!(
        blocked(actions::prepare_close(&chain, &organizer, &kept)),
        "NotTicketOwner"
    );
    chain.run(&holder, actions::prepare_close(&chain, &holder, &kept));
    assert!(
        model::ticket(&chain, &kept).is_err(),
        "the closed ticket is burned"
    );

    // Both live on in the event's history, with how they ended and who held them
    let mut history = History::in_memory();
    while history.needs_sync() {
        history.step(&chain, &event).unwrap();
    }
    let tickets = history::tickets(history.events());
    assert!(matches!(tickets[&refunded].fate, Fate::Refunded { .. }));
    assert!(matches!(tickets[&kept].fate, Fate::Closed { .. }));
    for asset in [refunded, kept] {
        assert_eq!(tickets[&asset].holder, Some(holder.pubkey()));
    }
    assert_eq!(tickets[&refunded].number(), Some(1));
    assert_eq!(tickets[&kept].number(), Some(2));
}

/// Several tickets mint in one transaction, all or nothing, within the wallet limit and the
/// supply.
#[test]
fn several_tickets_mint_in_one_transaction() {
    let mut chain = SvmChain::new();
    let organizer = chain.wallet(Some(0));
    let minter = chain.wallet(Some(100 * USDC));
    let event = chain
        .run(
            &organizer,
            actions::prepare_init_event(&chain, &organizer, new_event(1, vec![organizer.pubkey()])),
        )
        .created
        .unwrap();

    assert_eq!(
        blocked(actions::prepare_mint_many(&chain, &minter, &event, 4)),
        "MintLimitReached"
    );
    assert!(actions::prepare_mint_many(&chain, &minter, &event, 0).is_err());

    let plan = actions::prepare_mint_many(&chain, &minter, &event, 3).unwrap();
    assert_eq!(plan.title, "Mint 3 tickets");
    assert_eq!(plan.instructions.len(), 3);
    chain.run(&minter, Ok(plan));
    let info = model::event(&chain, &event).unwrap();
    let mine: Vec<_> = model::event_tickets(&chain, &info)
        .unwrap()
        .into_iter()
        .filter(|t| t.owner == minter.pubkey())
        .collect();
    assert_eq!(mine.len(), 3);
    assert_eq!(
        chain.usdc(&minter.pubkey()),
        100 * USDC - 3 * (PRICE + MINT_FEE)
    );
    assert_eq!(chain.usdc(&organizer.pubkey()), 3 * PRICE);
    assert_eq!(
        blocked(actions::prepare_mint(&chain, &minter, &event)),
        "MintLimitReached"
    );

    // Two tickets left: three is too many, even for a fresh wallet
    let small = chain
        .run(
            &organizer,
            actions::prepare_init_event(
                &chain,
                &organizer,
                NewEvent {
                    ticket_supply: 2,
                    ..new_event(2, vec![organizer.pubkey()])
                },
            ),
        )
        .created
        .unwrap();
    let fresh = chain.wallet(Some(100 * USDC));
    assert_eq!(
        blocked(actions::prepare_mint_many(&chain, &fresh, &small, 3)),
        "EventSoldOut"
    );
    chain.run(
        &fresh,
        actions::prepare_mint_many(&chain, &fresh, &small, 2),
    );
    assert_eq!(model::event(&chain, &small).unwrap().tickets_left(), 0);
}

/// "An hour from now" works the way people mean it: `+1h` counts from the moment you confirm
/// (however long the confirm screen stayed open), and an exact time too close to the 1-hour
/// minimum is refused before sending, with the earliest time that works.
#[test]
fn an_event_an_hour_from_now_lands() {
    use frost_pass_cli::ui::{draw, App};
    use ratatui::{
        backend::TestBackend,
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
        Terminal,
    };

    let mut chain = SvmChain::new();
    let organizer = chain.wallet(Some(0));

    // An exact end just over the minimum would fail on chain once the clock moves on
    let tight = NewEvent {
        sales_end: START + 600,
        event_end: START + 3_600 + 10,
        ..new_event(1, vec![organizer.pubkey()])
    };
    let Err(refused) = actions::prepare_init_event(&chain, &organizer, tight) else {
        panic!("an end 10 s past the minimum should be refused");
    };
    let violation = refused.downcast_ref::<Violation>().unwrap();
    assert_eq!(violation.rule, "EventDurationTooShort");
    assert!(
        violation.message.contains("or later"),
        "{}",
        violation.message
    );

    let mut app = App::new(chain, organizer, "litesvm".into()).without_animation();
    let mut terminal = Terminal::new(TestBackend::new(120, 42)).unwrap();
    let mut screen = |app: &mut App<SvmChain>| {
        while app.has_pending() || app.has_idle_work() {
            if app.has_pending() {
                app.run_pending();
            } else {
                app.run_idle();
            }
        }
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>() + "\n")
            .collect::<String>()
    };
    let key = |app: &mut App<SvmChain>, code: KeyCode, modifiers: KeyModifiers| {
        app.on_key(KeyEvent::new(code, modifiers))
    };
    let typed = |app: &mut App<SvmChain>, text: &str| {
        key(app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        for c in text.chars() {
            key(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        key(app, KeyCode::Tab, KeyModifiers::NONE);
    };

    screen(&mut app);
    key(&mut app, KeyCode::Char('1'), KeyModifiers::NONE);
    key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
    // Name, URI, price, supply, cap, royalty, sales end, event end
    for value in [
        "Last Minute Gig",
        "https://example.com/gig.json",
        "1",
        "10",
        "10",
        "5",
        "+30m",
        "+1h",
    ] {
        typed(&mut app, value);
    }
    key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(screen(&mut app).contains("Create event"));

    // Five minutes on the confirm screen: "+1h" still means an hour from confirming
    app.chain.set_time(START + 300);
    key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
    let done = screen(&mut app);
    assert!(done.contains("Create event confirmed"), "{done}");
    let event = app
        .events
        .iter()
        .find(|e| e.name == "Last Minute Gig")
        .unwrap();
    assert!(event.config.event_end >= START + 300 + 3_600);
}

/// Buying in the Marketplace takes the listing's price: no price to type, and the confirm
/// screen shows what's paid. Minting and passes point to the Ticket tab.
#[test]
fn the_marketplace_buys_at_the_listed_price() {
    use frost_pass_cli::ui::{draw, App};
    use ratatui::{
        backend::TestBackend,
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
        Terminal,
    };

    let mut chain = SvmChain::new();
    let organizer = chain.wallet(Some(0));
    let seller = chain.wallet(Some(100 * USDC));
    let buyer = chain.wallet(Some(100 * USDC));
    let event = chain
        .run(
            &organizer,
            actions::prepare_init_event(&chain, &organizer, new_event(4, vec![organizer.pubkey()])),
        )
        .created
        .unwrap();
    let ticket = chain
        .run(&seller, actions::prepare_mint(&chain, &seller, &event))
        .created
        .unwrap();
    let price = PRICE + PRICE / 20;
    chain.run(
        &seller,
        actions::prepare_list(&chain, &seller, &ticket, price),
    );

    let buyer_key = buyer.pubkey();
    let mut app = App::new(chain, buyer, "litesvm".into()).without_animation();
    let mut terminal = Terminal::new(TestBackend::new(120, 42)).unwrap();
    let mut screen = |app: &mut App<SvmChain>| {
        while app.has_pending() || app.has_idle_work() {
            if app.has_pending() {
                app.run_pending();
            } else {
                app.run_idle();
            }
        }
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>() + "\n")
            .collect::<String>()
    };
    let press = |app: &mut App<SvmChain>, code: KeyCode| {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    };

    press(&mut app, KeyCode::Char('3'));
    let market = screen(&mut app);
    assert!(
        market.contains("Marketplace") && market.contains("Listed · 10.50 USDC"),
        "{market}"
    );
    press(&mut app, KeyCode::Char('m'));
    assert!(screen(&mut app).contains("in the Ticket tab (2)"));

    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Char('b'));
    let confirm = screen(&mut app);
    assert!(
        confirm.contains("Buy ticket") && confirm.contains("10.50 USDC"),
        "straight to the confirm screen, at the listed price\n{confirm}"
    );
    press(&mut app, KeyCode::Char('y'));
    screen(&mut app);
    assert_eq!(app.chain.usdc(&buyer_key), 100 * USDC - price);
    assert_eq!(
        model::ticket(&app.chain, &ticket).unwrap().1.owner,
        buyer_key
    );
}

/// The app itself, driven by keys and a paste like a person would: each actor's tab renders,
/// a holder shows a door pass, and the scanner pastes it and redeems it with two key presses.
#[test]
fn the_app_walks_one_wallet_through_every_tab() {
    use frost_pass_cli::ui::{draw, App};
    use ratatui::{
        backend::TestBackend,
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
        Terminal,
    };

    let mut chain = SvmChain::new();
    // The app's wallet mints, holds and scans; the organizer is another wallet
    let organizer = chain.wallet(Some(0));
    let wallet = chain.wallet(Some(100 * USDC));
    // An older event, created first: the newer one lists above it
    chain.run(
        &organizer,
        actions::prepare_init_event(
            &chain,
            &organizer,
            NewEvent {
                name: "Older Event".into(),
                ..new_event(8, vec![organizer.pubkey()])
            },
        ),
    );
    let event = chain
        .run(
            &organizer,
            actions::prepare_init_event(&chain, &organizer, new_event(9, vec![wallet.pubkey()])),
        )
        .created
        .unwrap();
    let ticket = chain
        .run(&wallet, actions::prepare_mint(&chain, &wallet, &event))
        .created
        .unwrap();

    let mut app = App::new(chain, wallet, "litesvm".into()).without_animation();
    let mut terminal = Terminal::new(TestBackend::new(120, 42)).unwrap();
    let mut screen = |app: &mut App<SvmChain>| {
        while app.has_pending() || app.has_idle_work() {
            if app.has_pending() {
                app.run_pending();
            } else {
                app.run_idle();
            }
        }
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer
            .content
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>() + "\n")
            .collect();
        println!("{text}");
        text
    };
    let press = |app: &mut App<SvmChain>, code: KeyCode| {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    };

    let minter = screen(&mut app);
    assert!(minter.contains("Coldplay Devnet Tour"));
    assert!(minter.contains("Your tickets (1)"));
    assert!(minter.contains("you've minted 1 of 3"));
    assert!(
        minter.find("Coldplay Devnet Tour") < minter.find("Older Event"),
        "the newest event comes first\n{minter}"
    );

    // Search by event ID narrows the list; Esc brings everything back
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Char('z'));
    assert!(screen(&mut app).contains("No events match"));
    press(&mut app, KeyCode::Backspace);
    press(&mut app, KeyCode::Char('9'));
    press(&mut app, KeyCode::Enter);
    let found = screen(&mut app);
    assert!(
        found.contains("search: 9") && !found.contains("Older Event"),
        "{found}"
    );
    assert!(found.contains("Your tickets (1)"), "{found}");
    press(&mut app, KeyCode::Esc);
    assert!(screen(&mut app).contains("Older Event"));
    // By name, the way people search: every word, any order, any case
    press(&mut app, KeyCode::Char('/'));
    for c in "tour COLDPLAY".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    press(&mut app, KeyCode::Enter);
    let by_name = screen(&mut app);
    assert!(
        by_name.contains("Your tickets (1)") && !by_name.contains("Older Event"),
        "{by_name}"
    );
    press(&mut app, KeyCode::Esc);
    assert!(screen(&mut app).contains("Older Event"));

    // Mint two more in one go; then the wallet is at its limit
    press(&mut app, KeyCode::Char('m'));
    let form = screen(&mut app);
    assert!(
        form.contains("Mint · Coldplay Devnet Tour") && form.contains("1–2"),
        "{form}"
    );
    press(&mut app, KeyCode::Backspace);
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("Mint 2 tickets"));
    press(&mut app, KeyCode::Char('y'));
    let minted = screen(&mut app);
    assert!(minted.contains("Your tickets (3)"), "{minted}");
    press(&mut app, KeyCode::Char('m'));
    assert!(screen(&mut app).contains("[MintLimitReached]"));

    // The log has the transaction, with what the cluster recorded
    press(&mut app, KeyCode::Char('5'));
    let log = screen(&mut app);
    assert!(
        log.contains("Transactions (1)") && log.contains("FrostPass: mint_ticket"),
        "{log}"
    );
    let entry = &app.log.entries[0];
    assert_eq!(entry.action, "Mint 2 tickets");
    assert_eq!(entry.event_id, Some(9));
    assert_eq!(entry.instructions, vec!["FrostPass: mint_ticket"; 2]);
    assert!(entry.slot.is_some() && entry.compute_units.is_some());
    assert_eq!(entry.usdc_change, Some(-2 * (PRICE + MINT_FEE) as i64));
    // The details scroll: the last program log line is reachable
    assert!(log.contains("→ to scroll"), "{log}");
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::End);
    let bottom = screen(&mut app);
    let last_log = app.log.entries[0].logs.last().unwrap().clone();
    assert!(bottom.contains(&last_log), "{bottom}");
    press(&mut app, KeyCode::Home);
    assert!(screen(&mut app).contains("Status"));
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Char('8'));
    assert!(screen(&mut app).contains("No transactions match"));
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('1'));
    let organizer_tab = screen(&mut app);
    assert!(organizer_tab.contains("Your events") && organizer_tab.contains("No events yet"));
    press(&mut app, KeyCode::Char('n'));
    assert!(screen(&mut app).contains("Create event"));
    // A value that wraps over several rows keeps the key hints in view
    for _ in 0..200 {
        press(&mut app, KeyCode::Char('x'));
    }
    let long_form = screen(&mut app);
    assert!(long_form.contains("Esc cancel"), "{long_form}");
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('3'));
    assert!(screen(&mut app).contains("Listings and your tickets (3)"));

    // The holder's door pass
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('p'));
    let pass_screen = screen(&mut app);
    assert!(
        pass_screen.contains("Door pass") && pass_screen.contains("▀"),
        "the QR code fits"
    );
    assert!(pass_screen.contains("Esc close"), "{pass_screen}");
    // The text form wraps over several rows; the key hints stay in view
    press(&mut app, KeyCode::Char('t'));
    let pass_text = screen(&mut app);
    assert!(
        pass_text.contains("FROSTPASS:1:") && pass_text.contains("Esc close"),
        "{pass_text}"
    );
    let pass = app.shown_pass().unwrap();
    press(&mut app, KeyCode::Esc);

    // The scanner pastes it, sees every check pass, and redeems
    press(&mut app, KeyCode::Char('4'));
    app.on_paste(&pass);
    assert!(screen(&mut app).contains("ADMIT"));
    press(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("Redeem ticket"));
    press(&mut app, KeyCode::Char('y'));
    let door = screen(&mut app);
    assert!(
        door.contains("Door log · Coldplay Devnet Tour redeemed (1)")
            && door.contains("#1      you"),
        "{door}"
    );
    assert!(
        model::ticket(&app.chain, &ticket).is_err(),
        "the ticket is burned"
    );

    // The burned ticket stays visible, with how it ended: for its holder...
    press(&mut app, KeyCode::Char('2'));
    let holder = screen(&mut app);
    assert!(
        holder.contains("Your tickets (3)")
            && holder.contains("Redeemed")
            && holder.contains("by you"),
        "{holder}"
    );
    // ...nothing more can be done with it...
    press(&mut app, KeyCode::Char('p'));
    assert!(screen(&mut app).contains("is redeemed: there's nothing left to do with it"));

    // ...and for the organizer, with every other ticket
    let mut organizer_app = App::new(app.chain, organizer, "litesvm".into()).without_animation();
    press(&mut organizer_app, KeyCode::Char('1'));
    let all = screen(&mut organizer_app);
    assert!(
        all.contains("All tickets (3)") && all.contains("Redeemed"),
        "{all}"
    );
    assert_eq!(all.matches("Held").count(), 2, "{all}");
    // The organizer sees the door log too, without being a scanner
    assert!(
        all.contains("Door log · Coldplay Devnet Tour redeemed (1)"),
        "{all}"
    );

    // The door log takes focus: → from the tickets, or a click on one of its rows
    use frost_pass_cli::ui::{HitTarget, Pane, Tab};
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let click = |app: &mut App<SvmChain>, column: u16, row: u16| {
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    };
    press(&mut organizer_app, KeyCode::Right);
    press(&mut organizer_app, KeyCode::Right);
    assert_eq!(organizer_app.pane, Pane::DoorLog);
    press(&mut organizer_app, KeyCode::Left);
    assert_eq!(organizer_app.pane, Pane::Tickets);
    let door_row = organizer_app
        .hits
        .borrow()
        .iter()
        .find(|hit| hit.target == HitTarget::Pane(Pane::DoorLog) && hit.row_height == 1)
        .copied()
        .expect("the door log's rows are clickable");
    click(&mut organizer_app, door_row.area.x + 2, door_row.first_row);
    assert_eq!(
        (organizer_app.pane, organizer_app.door_index),
        (Pane::DoorLog, 0)
    );
    // Tabs switch with a click too
    let scanner_tab = organizer_app
        .hits
        .borrow()
        .iter()
        .find(|hit| hit.target == HitTarget::Tab(3))
        .copied()
        .unwrap();
    click(
        &mut organizer_app,
        scanner_tab.area.x + 1,
        scanner_tab.area.y,
    );
    assert_eq!(organizer_app.tab, Tab::Scanner);
}
