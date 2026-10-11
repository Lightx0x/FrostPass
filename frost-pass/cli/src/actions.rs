//! Every action of every actor, in two steps: `prepare_*` loads the current state, checks the
//! program's rules (see [`checks`]) and returns a [`Plan`] describing exactly what will happen;
//! [`execute`] signs and sends it. The app shows each plan for confirmation before sending.

use anchor_lang::{prelude::Pubkey, solana_program::instruction::Instruction};
use anchor_spl::{
    associated_token::spl_associated_token_account::instruction::create_associated_token_account_idempotent,
    token::ID as TOKEN_PROGRAM_ID,
};
use anyhow::{Context, Result};
use frost_pass::constants::{
    CHALLENGE_VALIDITY_SECONDS, MAX_MINTS_PER_WALLET, MAX_RESALE, MINT_FEE, PROTOCOL_TREASURY,
    USDC_MINT,
};
use frost_pass_client::{
    buy_ticket_ix, cancel_event_ix, close_ticket_ix, delist_ticket_ix, init_event_ix,
    list_ticket_ix, mint_ticket_ix, redeem_ticket_ix, refund_ticket_ix, update_scanners_ix, Event,
    InitEventArgs,
};
use solana_keypair::Keypair;
use solana_signature::Signature;
use solana_signer::Signer;

use crate::{
    chain::Chain,
    checks::{self, NewEvent, Violation},
    format,
    model::{self, EventInfo, TicketInfo},
    pass::Pass,
};

/// A transaction ready to send, with a plain description for the confirmation screen.
pub struct Plan {
    pub title: String,
    pub lines: Vec<(String, String)>,
    /// Shown prominently, for irreversible actions.
    pub warning: Option<String>,
    pub instructions: Vec<Instruction>,
    /// New accounts that must co-sign (an event's collection, a ticket's asset).
    pub new_accounts: Vec<Keypair>,
    /// The main account the action creates, if any.
    pub created: Option<Pubkey>,
}

impl Plan {
    fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lines: Vec::new(),
            warning: None,
            instructions: Vec::new(),
            new_accounts: Vec::new(),
            created: None,
        }
    }

    fn line(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.lines.push((label.into(), value.into()));
        self
    }
}

pub struct Receipt {
    pub title: String,
    pub signature: Signature,
    pub created: Option<Pubkey>,
}

/// Signs `plan` with `wallet` (the fee payer) and any new accounts, and sends it.
pub fn execute(chain: &mut impl Chain, wallet: &Keypair, plan: Plan) -> Result<Receipt> {
    let mut signers = vec![wallet];
    signers.extend(plan.new_accounts.iter());
    let signature = chain.send(&plan.instructions, &signers)?;
    Ok(Receipt {
        title: plan.title,
        signature,
        created: plan.created,
    })
}

fn violation(rule: &'static str, message: impl Into<String>) -> anyhow::Error {
    Violation {
        rule,
        message: message.into(),
    }
    .into()
}

fn create_usdc_account(payer: &Pubkey, owner: &Pubkey) -> Instruction {
    create_associated_token_account_idempotent(payer, owner, &USDC_MINT, &TOKEN_PROGRAM_ID)
}

fn ticket_line(ticket: &TicketInfo) -> (String, String) {
    (
        "Ticket".into(),
        format!("{} · {}", ticket.name, format::short(&ticket.asset)),
    )
}

// ---------------------------------------------------------------------------
// Organizer
// ---------------------------------------------------------------------------

pub fn prepare_init_event(chain: &impl Chain, organizer: &Keypair, new: NewEvent) -> Result<Plan> {
    let now = chain.now()?;
    let organizer_key = organizer.pubkey();
    let collection = Keypair::new();
    checks::new_event(&new, &organizer_key, &collection.pubkey(), now)?;

    let event = Event::new(&organizer_key, new.event_id, &collection.pubkey());
    if chain.account(&event.config)?.is_some() {
        return Err(violation(
            "EventIdTaken",
            format!("You already have an event with ID {}", new.event_id),
        ));
    }

    let cap = checks::resale_cap(&frost_pass::state::EventConfig {
        ticket_price: new.ticket_price,
        markup_cap_bps: new.markup_cap_bps,
        ..empty_config()
    });
    let mut plan = Plan::new("Create event")
        .line("Name", &new.name)
        .line("Event ID", new.event_id.to_string())
        .line(
            "Ticket price",
            format!("{} USDC", format::usdc(new.ticket_price)),
        )
        .line("Supply", format!("{} tickets", new.ticket_supply))
        .line(
            "Resale cap",
            format!(
                "{} USDC (face value + {})",
                format::usdc(u64::try_from(cap).unwrap_or(u64::MAX)),
                format::percent(new.markup_cap_bps)
            ),
        )
        .line(
            "Royalty",
            format!("{} of each resale", format::percent(new.royalty_bps)),
        )
        .line(
            "Sales end",
            format!(
                "{} ({})",
                format::datetime(new.sales_end),
                format::relative(new.sales_end, now)
            ),
        )
        .line(
            "Event ends",
            format!(
                "{} ({})",
                format::datetime(new.event_end),
                format::relative(new.event_end, now)
            ),
        )
        .line(
            "Scanners",
            new.scanners
                .iter()
                .map(format::short)
                .collect::<Vec<_>>()
                .join(", "),
        )
        .line(
            "Collection",
            "Frozen: tickets can only move through FrostPass",
        );

    if model::usdc_balance(chain, &organizer_key)?.is_none() {
        plan.instructions
            .push(create_usdc_account(&organizer_key, &organizer_key));
        plan = plan.line("Also", "Creates your USDC account to receive ticket sales");
    }
    plan.instructions.push(init_event_ix(
        &organizer_key,
        &event,
        InitEventArgs {
            event_id: new.event_id,
            name: new.name,
            uri: new.uri,
            ticket_price: new.ticket_price,
            ticket_supply: new.ticket_supply,
            markup_cap_bps: new.markup_cap_bps,
            royalty_bps: new.royalty_bps,
            sales_end: new.sales_end,
            event_end: new.event_end,
            scanners: new.scanners,
        },
    ));
    plan.created = Some(event.config);
    plan.new_accounts.push(collection);
    Ok(plan)
}

fn empty_config() -> frost_pass::state::EventConfig {
    frost_pass::state::EventConfig {
        event_id: 0,
        organizer: Pubkey::default(),
        collection: Pubkey::default(),
        ticket_price: 0,
        ticket_supply: 0,
        tickets_minted: 0,
        markup_cap_bps: 0,
        royalty_bps: 0,
        sales_end: 0,
        event_end: 0,
        cancelled: false,
        scanners: Vec::new(),
        bump: 0,
    }
}

pub fn prepare_update_scanners(
    chain: &impl Chain,
    organizer: &Keypair,
    event: &Pubkey,
    scanners: Vec<Pubkey>,
) -> Result<Plan> {
    let event = model::event(chain, event)?;
    checks::update_scanners(&event.config, &organizer.pubkey(), &scanners)?;
    let list = |keys: &[Pubkey]| {
        keys.iter()
            .map(format::short)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut plan = Plan::new("Update scanners")
        .line("Event", &event.name)
        .line("Current", list(&event.config.scanners))
        .line("New", list(&scanners));
    let removed: Vec<Pubkey> = event
        .config
        .scanners
        .iter()
        .filter(|s| !scanners.contains(s))
        .copied()
        .collect();
    if !removed.is_empty() {
        plan = plan.line("Revoked", list(&removed));
    }
    plan.instructions.push(update_scanners_ix(
        &event.event(),
        &organizer.pubkey(),
        scanners,
    ));
    Ok(plan)
}

pub fn prepare_cancel(chain: &impl Chain, organizer: &Keypair, event: &Pubkey) -> Result<Plan> {
    let event = model::event(chain, event)?;
    checks::cancel(&event.config, &organizer.pubkey())?;
    let mut plan = Plan::new("Cancel event").line("Event", &event.name).line(
        "Tickets out",
        format!(
            "{} (refund each at face value)",
            event.config.tickets_minted
        ),
    );
    plan.warning = Some(
        "This can't be undone. Minting, listing, buying and redeeming stop for good; \
         you can then refund holders at face value."
            .into(),
    );
    plan.instructions
        .push(cancel_event_ix(&event.event(), &organizer.pubkey()));
    Ok(plan)
}

pub fn prepare_refund(chain: &impl Chain, organizer: &Keypair, asset: &Pubkey) -> Result<Plan> {
    let (event, ticket) = model::ticket(chain, asset)?;
    let organizer_key = organizer.pubkey();
    checks::refund(&event.config, &organizer_key, &ticket.owner, &ticket.owner)?;
    checks::usdc_funds(
        model::usdc_balance(chain, &organizer_key)?,
        event.config.ticket_price,
        "Your wallet",
    )?;

    let (label, value) = ticket_line(&ticket);
    let mut plan = Plan::new("Refund ticket")
        .line(label, value)
        .line("Holder", ticket.owner.to_string())
        .line(
            "Refund",
            format!(
                "{} USDC (face value; the mint fee isn't refunded)",
                format::usdc(event.config.ticket_price)
            ),
        )
        .line("Ticket", "Burned; both rents go to the holder");
    if model::usdc_balance(chain, &ticket.owner)?.is_none() {
        plan.instructions
            .push(create_usdc_account(&organizer_key, &ticket.owner));
        plan = plan.line(
            "Also",
            "Creates the holder's USDC account (you pay its rent)",
        );
    }
    plan.instructions.push(refund_ticket_ix(
        &event.event(),
        &organizer_key,
        &ticket.owner,
        asset,
    ));
    Ok(plan)
}

// ---------------------------------------------------------------------------
// Minter
// ---------------------------------------------------------------------------

pub fn prepare_mint(chain: &impl Chain, minter: &Keypair, event: &Pubkey) -> Result<Plan> {
    let now = chain.now()?;
    let event = model::event(chain, event)?;
    let minter_key = minter.pubkey();
    let minted = model::mints_by(chain, &event.address, &minter_key)?;
    checks::mint(&event.config, &minter_key, minted, now)?;

    let total = event
        .config
        .ticket_price
        .checked_add(MINT_FEE)
        .context("price overflow")?;
    checks::usdc_funds(
        model::usdc_balance(chain, &minter_key)?,
        total,
        "Your wallet",
    )?;
    let accounts = model::usdc_accounts_exist(chain, &[event.config.organizer, PROTOCOL_TREASURY])?;
    checks::usdc_account(accounts[&event.config.organizer], "The organizer")?;
    checks::usdc_account(accounts[&PROTOCOL_TREASURY], "The protocol treasury")?;

    let asset = Keypair::new();
    let mut plan = Plan::new("Mint ticket")
        .line("Event", &event.name)
        .line(
            "Ticket",
            format!(
                "#{} of {}",
                event.config.tickets_minted + 1,
                event.config.ticket_supply
            ),
        )
        .line(
            "Price",
            format!(
                "{} USDC → organizer",
                format::usdc(event.config.ticket_price)
            ),
        )
        .line(
            "Mint fee",
            format!("{} USDC → protocol", format::usdc(MINT_FEE)),
        )
        .line("Total", format!("{} USDC", format::usdc(total)))
        .line(
            "Your mints",
            format!("{} of {MAX_MINTS_PER_WALLET} for this event", minted + 1),
        )
        .line(
            "Note",
            "The ticket is frozen in your wallet: it moves only through FrostPass resales",
        );
    plan.instructions
        .push(mint_ticket_ix(&event.event(), &minter_key, &asset.pubkey()));
    plan.created = Some(asset.pubkey());
    plan.new_accounts.push(asset);
    Ok(plan)
}

// ---------------------------------------------------------------------------
// Ticket holder (minter or buyer)
// ---------------------------------------------------------------------------

pub fn prepare_list(
    chain: &impl Chain,
    seller: &Keypair,
    asset: &Pubkey,
    price: u64,
) -> Result<Plan> {
    let now = chain.now()?;
    let (event, ticket) = model::ticket(chain, asset)?;
    let seller_key = seller.pubkey();
    checks::list(
        &event.config,
        &ticket.state,
        &ticket.owner,
        &seller_key,
        price,
        now,
    )?;
    let split = checks::split(&event.config, price)?;

    let (label, value) = ticket_line(&ticket);
    let mut plan = Plan::new("List ticket")
        .line(label, value)
        .line("Price", format!("{} USDC", format::usdc(price)))
        .line(
            "If it sells",
            format!(
                "you get {} USDC, the organizer {} USDC royalty",
                format::usdc(split.seller),
                format::usdc(split.royalty)
            ),
        )
        .line(
            "Resales",
            format!("{} of {MAX_RESALE} used", ticket.state.resale_count),
        )
        .line(
            "Note",
            "The ticket stays in your wallet until it sells, and still gets you in",
        );
    if model::usdc_balance(chain, &seller_key)?.is_none() {
        plan.instructions
            .push(create_usdc_account(&seller_key, &seller_key));
        plan = plan.line("Also", "Creates your USDC account so buyers can pay you");
    }
    plan.instructions
        .push(list_ticket_ix(&event.event(), &seller_key, asset, price));
    Ok(plan)
}

pub fn prepare_delist(chain: &impl Chain, seller: &Keypair, asset: &Pubkey) -> Result<Plan> {
    let (_, ticket) = model::ticket(chain, asset)?;
    checks::delist(&ticket.state, &seller.pubkey())?;
    let (label, value) = ticket_line(&ticket);
    let mut plan = Plan::new("Delist ticket").line(label, value).line(
        "Price",
        format!(
            "{} USDC (listing removed)",
            format::usdc(ticket.state.list_price)
        ),
    );
    plan.instructions
        .push(delist_ticket_ix(&seller.pubkey(), asset));
    Ok(plan)
}

pub fn prepare_close(chain: &impl Chain, user: &Keypair, asset: &Pubkey) -> Result<Plan> {
    let now = chain.now()?;
    let (event, ticket) = model::ticket(chain, asset)?;
    checks::close(&event.config, &ticket.owner, &user.pubkey(), now)?;
    let (label, value) = ticket_line(&ticket);
    let mut plan = Plan::new("Close ticket").line(label, value).line(
        "Rent",
        "Both the ticket's and its state's rent come back to you",
    );
    plan.warning =
        Some("The ticket is burned. Keep it instead if you want it as a keepsake.".into());
    plan.instructions
        .push(close_ticket_ix(&event.event(), &user.pubkey(), asset));
    Ok(plan)
}

/// Signs a fresh door pass for a ticket the holder owns.
pub fn issue_pass(
    chain: &impl Chain,
    holder: &Keypair,
    asset: &Pubkey,
) -> Result<(Pass, EventInfo, TicketInfo)> {
    let now = chain.now()?;
    let (event, ticket) = model::ticket(chain, asset)?;
    if ticket.owner != holder.pubkey() {
        return Err(violation("NotTicketOwner", "You don't own this ticket"));
    }
    if event.config.cancelled {
        return Err(violation("EventCancelled", "The event is cancelled"));
    }
    if now >= event.config.event_end {
        return Err(violation("EventEnded", "The event has ended"));
    }
    let pass = Pass::sign(holder, &event.address, asset, now)?;
    Ok((pass, event, ticket))
}

// ---------------------------------------------------------------------------
// Buyer
// ---------------------------------------------------------------------------

pub fn prepare_buy(
    chain: &impl Chain,
    buyer: &Keypair,
    asset: &Pubkey,
    max_price: u64,
) -> Result<Plan> {
    let now = chain.now()?;
    let (event, ticket) = model::ticket(chain, asset)?;
    let buyer_key = buyer.pubkey();
    let seller = ticket
        .state
        .seller
        .ok_or_else(|| violation("NotListed", "The ticket isn't listed"))?;
    checks::buy(
        &event.config,
        &ticket.state,
        &ticket.owner,
        &buyer_key,
        &seller,
        max_price,
        now,
    )?;
    let price = ticket.state.list_price;
    let split = checks::split(&event.config, price)?;
    checks::usdc_funds(
        model::usdc_balance(chain, &buyer_key)?,
        price,
        "Your wallet",
    )?;
    let accounts = model::usdc_accounts_exist(chain, &[seller, event.config.organizer])?;
    checks::usdc_account(accounts[&seller], "The seller")?;
    checks::usdc_account(accounts[&event.config.organizer], "The organizer")?;

    let (label, value) = ticket_line(&ticket);
    let mut plan = Plan::new("Buy ticket")
        .line("Event", &event.name)
        .line(label, value)
        .line("Price", format!("{} USDC", format::usdc(price)))
        .line(
            "Seller gets",
            format!("{} USDC", format::usdc(split.seller)),
        )
        .line(
            "Royalty",
            format!(
                "{} USDC → organizer ({})",
                format::usdc(split.royalty),
                format::percent(event.config.royalty_bps)
            ),
        )
        .line("Protocol fee", "None on resales")
        .line(
            "Resales",
            format!(
                "{} of {MAX_RESALE} after this",
                ticket.state.resale_count + 1
            ),
        );
    plan.instructions.push(buy_ticket_ix(
        &event.event(),
        &buyer_key,
        &seller,
        asset,
        max_price,
    ));
    Ok(plan)
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// One line of the scanner's checklist.
pub struct PassCheck {
    pub label: &'static str,
    pub result: Result<String, String>,
}

/// What the scanner sees for a pass: who, which ticket, and every rule redeeming depends on.
pub struct PassReport {
    pub pass: Pass,
    pub event: Option<EventInfo>,
    pub ticket: Option<TicketInfo>,
    pub checks: Vec<PassCheck>,
}

impl PassReport {
    pub fn admits(&self) -> bool {
        self.checks.iter().all(|check| check.result.is_ok())
    }
}

/// Reads a pass and checks it against the chain, without sending anything.
pub fn inspect_pass(chain: &impl Chain, scanner: &Pubkey, text: &str) -> Result<PassReport> {
    let pass = Pass::decode(text)?;
    let now = chain.now()?;
    let (event, ticket) = match model::ticket(chain, &pass.ticket_asset) {
        Ok((event, ticket)) => (event, ticket),
        Err(error) => {
            return Ok(PassReport {
                pass,
                event: None,
                ticket: None,
                checks: vec![PassCheck {
                    label: "Ticket",
                    result: Err(error.to_string()),
                }],
            })
        }
    };
    let config = &event.config;
    let check = |label, holds: bool, ok: String, err: &str| PassCheck {
        label,
        result: if holds { Ok(ok) } else { Err(err.to_string()) },
    };
    let checks = vec![
        check(
            "Holder's signature",
            pass.verify(&event.address),
            format!("signed by {}", format::short(&pass.user)),
            "doesn't match: the pass was altered, forged, or is for another event",
        ),
        check(
            "Scanner",
            config.scanners.contains(scanner),
            format!("this wallet scans {}", event.name),
            "this wallet isn't a scanner of the event",
        ),
        check(
            "Event status",
            !config.cancelled && now < config.event_end,
            format!("open, ends {}", format::relative(config.event_end, now)),
            if config.cancelled {
                "the event is cancelled"
            } else {
                "the event has ended"
            },
        ),
        check(
            "Pass freshness",
            now <= pass.expiry && pass.expiry <= now.saturating_add(CHALLENGE_VALIDITY_SECONDS),
            format!("expires {}", format::relative(pass.expiry, now)),
            "expired or valid too long: ask for a fresh pass",
        ),
        check(
            "Ownership",
            ticket.owner == pass.user,
            format!("{} holds {}", format::short(&ticket.owner), ticket.name),
            "the pass holder no longer owns this ticket",
        ),
    ];
    Ok(PassReport {
        pass,
        event: Some(event),
        ticket: Some(ticket),
        checks,
    })
}

/// Redeems a pass: the holder's signature goes in an Ed25519 instruction right before
/// `redeem_ticket`; the scanner pays the fee and the holder gets both rents back.
pub fn prepare_redeem(chain: &impl Chain, scanner: &Keypair, pass: &Pass) -> Result<Plan> {
    let now = chain.now()?;
    let (event, ticket) = model::ticket(chain, &pass.ticket_asset)?;
    if !pass.verify(&event.address) {
        return Err(violation(
            "InvalidChallenge",
            "The pass signature doesn't match the holder",
        ));
    }
    let scanner_key = scanner.pubkey();
    checks::redeem(
        &event.config,
        &scanner_key,
        &ticket.owner,
        &pass.user,
        pass.expiry,
        now,
    )?;

    let challenge = pass.challenge(&event.address);
    let (label, value) = ticket_line(&ticket);
    let mut plan = Plan::new("Redeem ticket")
        .line("Event", &event.name)
        .line(label, value)
        .line("Holder", pass.user.to_string())
        .line(
            "Pass",
            format!("expires {}", format::relative(pass.expiry, now)),
        )
        .line("Result", "Ticket burned; both rents go to the holder");
    plan.instructions = vec![
        challenge.ed25519_ix(&pass.user, &pass.signature),
        redeem_ticket_ix(
            &event.event(),
            &scanner_key,
            &pass.user,
            &pass.ticket_asset,
            &challenge,
        ),
    ];
    Ok(plan)
}
