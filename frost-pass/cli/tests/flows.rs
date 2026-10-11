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
use frost_pass_cli::{
    actions::{self, Plan},
    chain::{Chain, Filter, TxError},
    checks::{NewEvent, Violation},
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

struct SvmChain {
    svm: LiteSVM,
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
        let transaction = Transaction::new_signed_with_payer(
            instructions,
            Some(&signers[0].pubkey()),
            signers,
            self.svm.latest_blockhash(),
        );
        self.svm
            .send_transaction(transaction)
            .map(|meta| meta.signature)
            .map_err(|failed| TxError {
                message: format!("{:?}", failed.err),
                logs: failed.meta.logs,
            })
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
        let mut chain = Self { svm };
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
    chain.run(
        &organizer,
        actions::prepare_update_scanners(&chain, &organizer, &event, vec![new_scanner.pubkey()]),
    );

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

    let mut app = App::new(chain, wallet, "litesvm".into());
    let mut terminal = Terminal::new(TestBackend::new(120, 42)).unwrap();
    let mut screen = |app: &mut App<SvmChain>| {
        while app.has_pending() {
            app.run_pending();
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

    press(&mut app, KeyCode::Char('1'));
    let organizer_tab = screen(&mut app);
    assert!(organizer_tab.contains("Your events") && organizer_tab.contains("No events yet"));
    press(&mut app, KeyCode::Char('n'));
    assert!(screen(&mut app).contains("Create event"));
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('3'));
    assert!(screen(&mut app).contains("Listings and your tickets (1)"));

    // The holder's door pass
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('p'));
    let pass_screen = screen(&mut app);
    assert!(
        pass_screen.contains("Door pass") && pass_screen.contains("▀"),
        "the QR code fits"
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
    assert!(door.contains("Redeemed this session (1)"), "{door}");
    assert!(
        model::ticket(&app.chain, &ticket).is_err(),
        "the ticket is burned"
    );
}
