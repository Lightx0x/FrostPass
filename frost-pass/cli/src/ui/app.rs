//! App state and input handling. Network calls never run inside a key handler: the handler
//! queues a [`Job`] and sets a busy status, the loop draws that, then runs the job.

use std::{
    cell::Cell,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anchor_lang::prelude::Pubkey;
use anyhow::Result;
use frost_pass::constants::{MAX_MARKUP_BPS, MAX_ROYALTY_BPS, MAX_SCANNERS};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use solana_keypair::Keypair;
use solana_signature::Signature;
use solana_signer::Signer;

use super::form::{Field, Form, FormInput, FormKind};
use crate::{
    actions::{self, PassReport, Plan},
    chain::{Chain, TxError},
    checks::{self, NewEvent, Violation},
    format,
    model::{self, EventInfo, TicketInfo},
    pass::Pass,
    store::{LogEntry, SlotCache, TxLog},
};

use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Organizer,
    Minter,
    Buyer,
    Scanner,
    /// This wallet's transactions.
    Log,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Organizer,
        Tab::Minter,
        Tab::Buyer,
        Tab::Scanner,
        Tab::Log,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Organizer => "Organizer",
            Tab::Minter => "Minter",
            Tab::Buyer => "Buyer",
            Tab::Scanner => "Scanner",
            Tab::Log => "Log",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|tab| *tab == self).unwrap()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Events,
    Tickets,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Busy,
    Ok,
    Error,
}

pub struct Status {
    pub kind: StatusKind,
    pub text: String,
}

pub struct Confirm {
    pub plan: Plan,
    /// For irreversible actions: the word to type, and what's typed so far.
    pub typed: Option<(&'static str, String)>,
    after: After,
}

/// Follow-up once a confirmed transaction lands.
enum After {
    Nothing,
    Scanned { ticket: String, holder: Pubkey },
}

pub struct PassView {
    pub pass: Pass,
    pub asset: Pubkey,
    pub ticket: String,
    /// Show the pass as text instead of a QR code.
    pub as_text: bool,
}

pub enum Modal {
    Form(Form),
    Confirm(Confirm),
    Pass(PassView),
    Help,
}

pub struct ScanRecord {
    pub time: i64,
    pub ticket: String,
    pub holder: Pubkey,
    pub signature: Signature,
}

enum Job {
    Refresh,
    RefreshLog,
    LoadTickets,
    Submit(Form),
    Prepare(Request),
    Execute(Confirm),
    IssuePass(Pubkey),
}

enum Request {
    Mint(Pubkey),
    Delist(Pubkey),
    Close(Pubkey),
    Cancel(Pubkey),
    Refund(Pubkey),
    Redeem,
}

pub struct App<C: Chain> {
    pub chain: C,
    pub wallet: Keypair,
    pub cluster: String,
    pub tab: Tab,
    pub pane: Pane,
    /// Cluster time minus local time, so countdowns follow the cluster clock between refreshes.
    clock_offset: i64,
    pub sol: u64,
    pub usdc: Option<u64>,
    pub events: Vec<EventInfo>,
    pub event_index: usize,
    pub tickets: Vec<TicketInfo>,
    pub ticket_index: usize,
    /// Primary mints by this wallet for the selected event.
    pub mints: u8,
    pub modal: Option<Modal>,
    pub status: Status,
    pub report: Option<PassReport>,
    pub scans: Vec<ScanRecord>,
    /// The search box (`/`): filters events by ID or name, and the log, on every tab.
    pub search: String,
    /// Typing goes to the search box.
    pub searching: bool,
    /// When each event was created, for listing newest first.
    slots: SlotCache,
    /// Events whose creation slot the cluster couldn't give; not asked again this run.
    slot_misses: HashSet<Pubkey>,
    /// Some events' creation slots are still being looked up (between key presses), so the
    /// order isn't final yet.
    pub ordering: bool,
    pub log: TxLog,
    pub log_index: usize,
    /// How far the selected transaction's details are scrolled (rows).
    pub log_scroll: u16,
    /// Set by each draw: the furthest the details can scroll, and the rows on screen.
    pub log_scroll_max: Cell<u16>,
    pub log_page: Cell<u16>,
    pending: Option<Job>,
    /// What to copy (`"Pass"`) and the text; the run loop copies it, then calls [`Self::copied`].
    pub clipboard: Option<(&'static str, String)>,
    pub quit: bool,
}

fn local_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A one-line explanation of why something failed.
pub fn describe(error: &anyhow::Error) -> String {
    if let Some(violation) = error.downcast_ref::<Violation>() {
        format!("Blocked: {} [{}]", violation.message, violation.rule)
    } else if let Some(tx) = error.downcast_ref::<TxError>() {
        format!("Transaction failed: {tx}")
    } else {
        format!("{error:#}")
    }
}

impl<C: Chain> App<C> {
    pub fn new(chain: C, wallet: Keypair, cluster: String) -> Self {
        Self {
            chain,
            wallet,
            cluster,
            tab: Tab::Minter,
            pane: Pane::Events,
            clock_offset: 0,
            sol: 0,
            usdc: None,
            events: Vec::new(),
            event_index: 0,
            tickets: Vec::new(),
            ticket_index: 0,
            mints: 0,
            modal: None,
            status: Status {
                kind: StatusKind::Busy,
                text: "Loading events…".into(),
            },
            report: None,
            scans: Vec::new(),
            search: String::new(),
            searching: false,
            slots: SlotCache::in_memory(),
            slot_misses: HashSet::new(),
            ordering: false,
            log: TxLog::in_memory(),
            log_index: 0,
            log_scroll: 0,
            log_scroll_max: Cell::new(0),
            log_page: Cell::new(10),
            pending: Some(Job::Refresh),
            clipboard: None,
            quit: false,
        }
    }

    /// Keeps the event order and this wallet's transaction log in `dir` (see [`crate::store`]).
    pub fn with_store(mut self, dir: &Path) -> Self {
        self.slots = SlotCache::open(dir);
        self.log = TxLog::open(dir, &self.me());
        self
    }

    /// Cluster time, estimated from the last refresh.
    pub fn now(&self) -> i64 {
        local_now() + self.clock_offset
    }

    pub fn me(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    /// Reports a copy: `tool` is the clipboard command that took it, if any.
    pub fn copied(&mut self, what: &str, tool: Option<&str>) {
        match tool {
            Some(tool) => self.done(format!("{what} copied to the clipboard ({tool})")),
            None => self.done(format!(
                "{what} sent to the terminal's clipboard (OSC 52); in tmux, Ctrl+b ] pastes it"
            )),
        }
    }

    /// The door pass on screen, as text.
    pub fn shown_pass(&self) -> Option<String> {
        match &self.modal {
            Some(Modal::Pass(view)) => Some(view.pass.encode()),
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // What each tab shows
    // -----------------------------------------------------------------------

    /// Organizers see their own events, scanners the events they scan; minters and buyers all.
    /// The search box narrows any of them.
    pub fn visible_events(&self) -> Vec<&EventInfo> {
        let me = self.me();
        self.events
            .iter()
            .filter(|event| match self.tab {
                Tab::Organizer => event.config.organizer == me,
                Tab::Scanner => event.config.scanners.contains(&me),
                Tab::Minter | Tab::Buyer | Tab::Log => true,
            })
            .filter(|event| matches_search(event, &self.search))
            .collect()
    }

    /// This wallet's transactions, newest first, narrowed by the search box.
    pub fn visible_log(&self) -> Vec<&LogEntry> {
        let query = self.search.trim().trim_start_matches('#');
        self.log
            .entries
            .iter()
            .rev()
            .filter(|entry| query.is_empty() || entry.matches(query))
            .collect()
    }

    pub fn selected_log(&self) -> Option<&LogEntry> {
        self.visible_log().get(self.log_index).copied()
    }

    pub fn selected_event(&self) -> Option<&EventInfo> {
        self.visible_events().get(self.event_index).copied()
    }

    /// Organizers see every ticket, minters their own, buyers listings and their own.
    pub fn visible_tickets(&self) -> Vec<&TicketInfo> {
        let me = self.me();
        self.tickets
            .iter()
            .filter(|ticket| match self.tab {
                Tab::Organizer => true,
                Tab::Minter => ticket.owner == me,
                Tab::Buyer => ticket.owner == me || ticket.state.seller.is_some(),
                Tab::Scanner | Tab::Log => false,
            })
            .collect()
    }

    pub fn selected_ticket(&self) -> Option<&TicketInfo> {
        self.visible_tickets().get(self.ticket_index).copied()
    }

    // -----------------------------------------------------------------------
    // Input
    // -----------------------------------------------------------------------

    pub fn on_paste(&mut self, text: &str) {
        if self.searching {
            self.search.push_str(text.trim());
            self.search_changed();
            return;
        }
        match &mut self.modal {
            Some(Modal::Form(form)) => form.paste(text),
            None if self.tab == Tab::Scanner => {
                let mut form = scan_form();
                form.paste(text);
                self.queue(Job::Submit(form), "Checking pass…");
            }
            _ => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.searching {
            self.on_search_key(key);
            return;
        }
        match self.modal.take() {
            Some(modal) => self.on_modal_key(modal, key),
            None => self.on_main_key(key),
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.searching = false;
                self.queue(Job::LoadTickets, "Loading tickets…");
            }
            KeyCode::Esc => {
                self.searching = false;
                self.search.clear();
                self.search_changed();
                self.queue(Job::LoadTickets, "Loading tickets…");
            }
            KeyCode::Backspace => {
                self.search.pop();
                self.search_changed();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.clear();
                self.search_changed();
            }
            KeyCode::Char(c) => {
                self.search.push(c);
                self.search_changed();
            }
            _ => {}
        }
    }

    /// The filter changed: start from the top. Tickets reload once the search is done, not
    /// on every key.
    fn search_changed(&mut self) {
        self.event_index = 0;
        self.ticket_index = 0;
        self.log_index = 0;
        self.log_scroll = 0;
        self.tickets.clear();
        self.mints = 0;
    }

    fn on_modal_key(&mut self, modal: Modal, key: KeyEvent) {
        match modal {
            Modal::Form(mut form) => match form.handle_key(key) {
                FormInput::Editing => self.modal = Some(Modal::Form(form)),
                FormInput::Cancel => self.info("Cancelled"),
                FormInput::Submit => self.queue(Job::Submit(form), "Checking…"),
            },
            Modal::Confirm(mut confirm) => match (&mut confirm.typed, key.code) {
                (_, KeyCode::Esc) | (None, KeyCode::Char('n')) => self.info("Cancelled"),
                (None, KeyCode::Enter | KeyCode::Char('y')) => {
                    let title = confirm.plan.title.clone();
                    self.queue(Job::Execute(confirm), &format!("{title}: sending…"));
                }
                (Some((word, typed)), KeyCode::Enter) if typed == word => {
                    let title = confirm.plan.title.clone();
                    self.queue(Job::Execute(confirm), &format!("{title}: sending…"));
                }
                (Some((_, typed)), KeyCode::Backspace) => {
                    typed.pop();
                    self.modal = Some(Modal::Confirm(confirm));
                }
                (Some((_, typed)), KeyCode::Char(c)) => {
                    typed.push(c);
                    self.modal = Some(Modal::Confirm(confirm));
                }
                _ => self.modal = Some(Modal::Confirm(confirm)),
            },
            Modal::Pass(mut view) => match key.code {
                KeyCode::Char('r') => self.queue(Job::IssuePass(view.asset), "Signing a new pass…"),
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {}
                code => {
                    match code {
                        KeyCode::Char('t') => view.as_text = !view.as_text,
                        KeyCode::Char('c') => {
                            self.clipboard = Some(("Pass", view.pass.encode()));
                        }
                        _ => {}
                    }
                    self.modal = Some(Modal::Pass(view));
                }
            },
            Modal::Help => {}
        }
    }

    fn on_main_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            KeyCode::Char('/') => {
                self.searching = true;
                self.info("Search: type an event name (or ID) · Enter keeps it · Esc clears it");
            }
            KeyCode::Char('r') if self.tab == Tab::Log => {
                self.queue(Job::RefreshLog, "Checking confirmations…")
            }
            KeyCode::Char('r') => self.queue(Job::Refresh, "Refreshing…"),
            KeyCode::Tab => self.switch_tab((self.tab.index() + 1) % Tab::ALL.len()),
            KeyCode::BackTab => {
                self.switch_tab((self.tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len())
            }
            KeyCode::Char(c @ '1'..='5') => self.switch_tab(c as usize - '1' as usize),
            KeyCode::Left => self.pane = Pane::Events,
            // In the Log, the right pane is the selected transaction's details
            KeyCode::Right if self.tab != Tab::Scanner => self.pane = Pane::Tickets,
            KeyCode::PageUp if self.tab == Tab::Log => {
                self.scroll_log(-(self.log_page.get() as isize))
            }
            KeyCode::PageDown if self.tab == Tab::Log => {
                self.scroll_log(self.log_page.get() as isize)
            }
            KeyCode::Home if self.tab == Tab::Log => self.log_scroll = 0,
            KeyCode::End if self.tab == Tab::Log => self.log_scroll = self.log_scroll_max.get(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            _ => self.on_action_key(key.code),
        }
    }

    fn on_action_key(&mut self, code: KeyCode) {
        let event = self.selected_event().map(|e| e.address);
        let ticket = self.selected_ticket().map(|t| t.asset);
        let need_event = "Select an event first";
        let need_ticket = "Select a ticket first (→ to move to the tickets pane)";
        match (self.tab, code) {
            // Organizer
            (Tab::Organizer, KeyCode::Char('n')) => {
                self.modal = Some(Modal::Form(self.new_event_form()))
            }
            (Tab::Organizer, KeyCode::Char('s')) => match self.selected_event() {
                Some(event) => self.modal = Some(Modal::Form(scanners_form(event))),
                None => self.error(need_event),
            },
            (Tab::Organizer, KeyCode::Char('c')) => match event {
                Some(event) => self.queue(Job::Prepare(Request::Cancel(event)), "Checking…"),
                None => self.error(need_event),
            },
            (Tab::Organizer, KeyCode::Char('f')) => match ticket {
                Some(ticket) => self.queue(Job::Prepare(Request::Refund(ticket)), "Checking…"),
                None => self.error(need_ticket),
            },

            // Minter
            (Tab::Minter, KeyCode::Char('m')) => match self.selected_event() {
                // Nothing left to mint: let the checks say why
                Some(event) if actions::mintable(event, self.mints) == 0 => {
                    self.queue(Job::Prepare(Request::Mint(event.address)), "Checking…")
                }
                Some(event) => self.modal = Some(Modal::Form(mint_form(event, self.mints))),
                None => self.error(need_event),
            },

            // Buyer
            (Tab::Buyer, KeyCode::Char('b')) => match self.selected_ticket() {
                Some(ticket) => self.modal = Some(Modal::Form(buy_form(ticket))),
                None => self.error(need_ticket),
            },

            // Holder actions, for minters and buyers alike
            (Tab::Minter | Tab::Buyer, KeyCode::Char('l')) => {
                match (self.selected_event(), self.selected_ticket()) {
                    (Some(event), Some(ticket)) => {
                        self.modal = Some(Modal::Form(list_form(event, ticket)))
                    }
                    _ => self.error(need_ticket),
                }
            }
            (Tab::Minter | Tab::Buyer, KeyCode::Char('d')) => match ticket {
                Some(ticket) => self.queue(Job::Prepare(Request::Delist(ticket)), "Checking…"),
                None => self.error(need_ticket),
            },
            (Tab::Minter | Tab::Buyer, KeyCode::Char('p')) => match ticket {
                Some(ticket) => self.queue(Job::IssuePass(ticket), "Signing your pass…"),
                None => self.error(need_ticket),
            },
            (Tab::Minter | Tab::Buyer, KeyCode::Char('x')) => match ticket {
                Some(ticket) => self.queue(Job::Prepare(Request::Close(ticket)), "Checking…"),
                None => self.error(need_ticket),
            },

            // Scanner
            (Tab::Scanner, KeyCode::Enter) if self.report.as_ref().is_some_and(|r| r.admits()) => {
                self.queue(Job::Prepare(Request::Redeem), "Checking…")
            }
            (Tab::Scanner, KeyCode::Char('v') | KeyCode::Enter) => {
                self.modal = Some(Modal::Form(scan_form()))
            }
            (Tab::Scanner, KeyCode::Esc) if self.report.is_some() => self.report = None,

            // Log
            (Tab::Log, KeyCode::Char('c')) => {
                match self.selected_log().and_then(|e| e.signature.clone()) {
                    Some(signature) => {
                        self.clipboard = Some(("Signature", signature));
                    }
                    None => self.error("This transaction has no signature (it never landed)"),
                }
            }

            (_, KeyCode::Esc) if !self.search.is_empty() => {
                self.search.clear();
                self.search_changed();
                self.queue(Job::LoadTickets, "Loading tickets…");
            }
            _ => {}
        }
    }

    fn switch_tab(&mut self, index: usize) {
        let previous = self.selected_event().map(|e| e.address);
        self.tab = Tab::ALL[index];
        self.pane = Pane::Events;
        self.ticket_index = 0;
        // Keep the same event selected when the new tab shows it
        let events = self.visible_events();
        self.event_index = previous
            .and_then(|address| events.iter().position(|e| e.address == address))
            .unwrap_or(0);
        if self.selected_event().map(|e| e.address) != previous {
            self.queue(Job::LoadTickets, "Loading tickets…");
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.tab == Tab::Log {
            match self.pane {
                Pane::Events => self.move_log_selection(delta),
                Pane::Tickets => self.scroll_log(delta),
            }
            return;
        }
        let step = |index: usize, len: usize| {
            (index as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize
        };
        match self.pane {
            Pane::Events => {
                let next = step(self.event_index, self.visible_events().len());
                if next != self.event_index {
                    self.event_index = next;
                    self.ticket_index = 0;
                    self.queue(Job::LoadTickets, "Loading tickets…");
                }
            }
            Pane::Tickets => {
                self.ticket_index = step(self.ticket_index, self.visible_tickets().len())
            }
        }
    }

    fn move_log_selection(&mut self, delta: isize) {
        let len = self.visible_log().len();
        let next =
            (self.log_index as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
        if next != self.log_index {
            self.log_index = next;
            self.log_scroll = 0;
        }
    }

    fn scroll_log(&mut self, delta: isize) {
        let max = self.log_scroll_max.get() as isize;
        self.log_scroll = (self.log_scroll as isize + delta).clamp(0, max) as u16;
    }

    fn new_event_form(&self) -> Form {
        let mut id = [0u8; 4];
        let _ = getrandom::getrandom(&mut id);
        Form::new(
            FormKind::NewEvent,
            "Create event",
            vec![
                Field::new("Name", "", "Shown on every ticket, at most 32 bytes"),
                Field::new(
                    "Metadata URI",
                    "",
                    "JSON metadata (image etc.), at most 200 bytes",
                ),
                Field::new("Ticket price (USDC)", "10", "Above 0"),
                Field::new("Supply", "100", "Number of tickets"),
                Field::new(
                    "Resale cap (%)",
                    "10",
                    format!(
                        "Highest resale price above face value, 0–{}",
                        format::percent(MAX_MARKUP_BPS)
                    ),
                ),
                Field::new(
                    "Royalty (%)",
                    "5",
                    format!(
                        "Your share of each resale, 0–{}",
                        format::percent(MAX_ROYALTY_BPS)
                    ),
                ),
                Field::new(
                    "Sales end",
                    "+1d",
                    "+90m, +2h, +3d or YYYY-MM-DD HH:MM (UTC)",
                ),
                Field::new(
                    "Event ends",
                    "+2d",
                    "At least 1 hour away; tickets redeem until then",
                ),
                Field::new(
                    "Scanners",
                    self.me().to_string(),
                    format!("Door wallets, comma-separated, 1–{MAX_SCANNERS}"),
                ),
                Field::new(
                    "Event ID",
                    (u32::from_le_bytes(id) % 1_000_000).to_string(),
                    "Any unused number",
                ),
            ],
        )
    }

    // -----------------------------------------------------------------------
    // Jobs
    // -----------------------------------------------------------------------

    fn queue(&mut self, job: Job, busy: &str) {
        self.pending = Some(job);
        self.status = Status {
            kind: StatusKind::Busy,
            text: busy.into(),
        };
    }

    fn info(&mut self, text: &str) {
        self.status = Status {
            kind: StatusKind::Info,
            text: text.into(),
        };
    }

    fn error(&mut self, text: impl Into<String>) {
        self.status = Status {
            kind: StatusKind::Error,
            text: text.into(),
        };
    }

    fn done(&mut self, text: impl Into<String>) {
        self.status = Status {
            kind: StatusKind::Ok,
            text: text.into(),
        };
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Runs the queued job, if any. Failures end up in the status line (or the form).
    pub fn run_pending(&mut self) {
        let Some(job) = self.pending.take() else {
            return;
        };
        let result = match job {
            Job::Refresh => self.refresh().map(|()| self.info("Up to date")),
            Job::RefreshLog => self.refresh_log(),
            Job::LoadTickets => self.load_tickets().map(|()| self.info("")),
            Job::Submit(form) => {
                self.submit(form);
                Ok(())
            }
            Job::Prepare(request) => self.prepare(request),
            Job::Execute(confirm) => self.execute(confirm),
            Job::IssuePass(asset) => self.issue_pass(&asset),
        };
        if let Err(error) = result {
            self.error(describe(&error));
        }
    }

    fn refresh(&mut self) -> Result<()> {
        let now = self.chain.now()?;
        self.clock_offset = now - local_now();
        let me = self.me();
        self.sol = model::sol_balance(&self.chain, &me)?;
        self.usdc = model::usdc_balance(&self.chain, &me)?;
        let selected = self.selected_event().map(|e| e.address);
        self.events = model::events(&self.chain, None)?;
        self.sort_events();
        let events = self.visible_events();
        self.event_index = selected
            .and_then(|address| events.iter().position(|e| e.address == address))
            .unwrap_or(self.event_index.min(events.len().saturating_sub(1)));
        self.load_tickets()
    }

    /// Sorts events newest first by the slot they were created in, as far as it's known. Unknown
    /// ones are looked up between key presses ([`Self::run_idle`]) and keep their place last.
    fn sort_events(&mut self) {
        let slots = &self.slots;
        model::sort_newest_first(&mut self.events, |address| slots.get(address));
        self.ordering = self.next_unknown_slot().is_some();
    }

    fn next_unknown_slot(&self) -> Option<Pubkey> {
        self.events
            .iter()
            .map(|event| event.address)
            .find(|address| {
                self.slots.get(address).is_none() && !self.slot_misses.contains(address)
            })
    }

    /// Background work for when no key is pressed: looks up one event's creation slot (a single
    /// RPC call, so input never waits long) and re-sorts, keeping the selection.
    pub fn has_idle_work(&self) -> bool {
        self.ordering && self.pending.is_none()
    }

    pub fn run_idle(&mut self) {
        let Some(address) = self.next_unknown_slot() else {
            self.ordering = false;
            return;
        };
        match self.chain.first_slot(&address) {
            Ok(Some(slot)) => {
                self.slots.insert(address, slot);
                let _ = self.slots.save();
            }
            Ok(None) => {
                self.slot_misses.insert(address);
            }
            // Rate limited or offline: keep the order we have; r tries again
            Err(_) => {
                self.ordering = false;
                return;
            }
        }
        let selected = self.selected_event().map(|e| e.address);
        self.sort_events();
        if self.event_index == 0 {
            // Still at the top: follow the top, which is the newest event as far as known
            if self.selected_event().map(|e| e.address) != selected {
                self.ticket_index = 0;
                self.queue(Job::LoadTickets, "Loading tickets…");
            }
        } else if let Some(index) = selected.and_then(|address| {
            self.visible_events()
                .iter()
                .position(|e| e.address == address)
        }) {
            self.event_index = index;
        }
    }

    /// Adds what the cluster recorded to transactions that don't have it yet, and marks
    /// finalized ones.
    fn refresh_log(&mut self) -> Result<()> {
        let me = self.me();
        let mut changed = false;
        let unsettled: Vec<usize> = (0..self.log.entries.len())
            .filter(|&i| self.log.entries[i].unsettled())
            .collect();
        let signatures: Vec<Signature> = unsettled
            .iter()
            .filter_map(|&i| self.log.entries[i].signature())
            .collect();
        let confirmations = self.chain.confirmations(&signatures)?;
        for (&i, confirmation) in unsettled.iter().zip(confirmations) {
            let entry = &mut self.log.entries[i];
            if entry.slot.is_none() {
                if let Some(signature) = entry.signature() {
                    if let Some(details) = self.chain.transaction(&signature, &me)? {
                        entry.add_details(details);
                        changed = true;
                    }
                }
            }
            if let Some(confirmation) = confirmation {
                let before = entry.status;
                entry.confirm(confirmation);
                changed |= entry.status != before;
            }
        }
        if changed {
            self.log.save()?;
        }
        let pending = self.log.entries.iter().filter(|e| e.unsettled()).count();
        if pending == 0 {
            self.info("Log up to date: every transaction is finalized");
        } else {
            self.info(&format!(
                "{pending} transaction(s) not finalized yet: press r again in a few seconds"
            ));
        }
        Ok(())
    }

    fn load_tickets(&mut self) -> Result<()> {
        let Some(event) = self.selected_event().cloned() else {
            self.tickets.clear();
            return Ok(());
        };
        self.tickets = model::event_tickets(&self.chain, &event)?;
        self.mints = model::mints_by(&self.chain, &event.address, &self.me())?;
        self.ticket_index = self
            .ticket_index
            .min(self.visible_tickets().len().saturating_sub(1));
        Ok(())
    }

    fn confirm(&mut self, plan: Plan) {
        self.modal = Some(Modal::Confirm(Confirm {
            plan,
            typed: None,
            after: After::Nothing,
        }));
        self.info("Review and confirm");
    }

    fn submit(&mut self, mut form: Form) {
        let result = match &form.kind {
            FormKind::NewEvent => self
                .parse_new_event(&form)
                .and_then(|new| actions::prepare_init_event(&self.chain, &self.wallet, new)),
            FormKind::Scanners { event } => format::parse_pubkeys(form.value(0)).and_then(|list| {
                actions::prepare_update_scanners(&self.chain, &self.wallet, event, list)
            }),
            FormKind::Mint { event } => parse_count(form.value(0)).and_then(|count| {
                actions::prepare_mint_many(&self.chain, &self.wallet, event, count)
            }),
            FormKind::List { asset } => format::parse_usdc(form.value(0))
                .and_then(|price| actions::prepare_list(&self.chain, &self.wallet, asset, price)),
            FormKind::Buy { asset } => format::parse_usdc(form.value(0))
                .and_then(|max| actions::prepare_buy(&self.chain, &self.wallet, asset, max)),
            FormKind::ScanPass => {
                match actions::inspect_pass(&self.chain, &self.me(), form.value(0)) {
                    Ok(report) => {
                        if report.admits() {
                            self.done("Pass valid: press Enter to redeem");
                        } else {
                            self.error("Pass refused: see the checks");
                        }
                        self.report = Some(report);
                    }
                    Err(error) => {
                        form.error = Some(describe(&error));
                        self.modal = Some(Modal::Form(form));
                        self.error("Couldn't read the pass");
                    }
                }
                return;
            }
        };
        match result {
            Ok(plan) => self.confirm(plan),
            Err(error) => {
                form.error = Some(describe(&error));
                self.modal = Some(Modal::Form(form));
                self.error("Fix the highlighted problem");
            }
        }
    }

    fn parse_new_event(&self, form: &Form) -> Result<NewEvent> {
        let now = self.now();
        // Errors name the field they came from
        fn field<T>(form: &Form, index: usize, parsed: Result<T>) -> Result<T> {
            parsed.map_err(|error| error.context(form.fields[index].label))
        }
        let number = |index: usize| {
            form.value(index)
                .parse::<u32>()
                .map_err(anyhow::Error::from)
        };
        Ok(NewEvent {
            name: form.value(0).to_string(),
            uri: form.value(1).to_string(),
            ticket_price: field(form, 2, format::parse_usdc(form.value(2)))?,
            ticket_supply: field(form, 3, number(3))?,
            markup_cap_bps: field(form, 4, format::parse_percent(form.value(4)))?,
            royalty_bps: field(form, 5, format::parse_percent(form.value(5)))?,
            sales_end: field(form, 6, format::parse_time(form.value(6), now))?,
            event_end: field(form, 7, format::parse_time(form.value(7), now))?,
            scanners: field(form, 8, format::parse_pubkeys(form.value(8)))?,
            event_id: field(form, 9, number(9))?,
        })
    }

    fn prepare(&mut self, request: Request) -> Result<()> {
        let chain = &self.chain;
        let wallet = &self.wallet;
        let (plan, typed, after) = match request {
            Request::Mint(event) => (
                actions::prepare_mint(chain, wallet, &event)?,
                None,
                After::Nothing,
            ),
            Request::Delist(asset) => (
                actions::prepare_delist(chain, wallet, &asset)?,
                None,
                After::Nothing,
            ),
            Request::Close(asset) => (
                actions::prepare_close(chain, wallet, &asset)?,
                None,
                After::Nothing,
            ),
            Request::Refund(asset) => (
                actions::prepare_refund(chain, wallet, &asset)?,
                None,
                After::Nothing,
            ),
            Request::Cancel(event) => (
                actions::prepare_cancel(chain, wallet, &event)?,
                Some(("cancel", String::new())),
                After::Nothing,
            ),
            Request::Redeem => {
                let report = self
                    .report
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Scan a pass first"))?;
                let plan = actions::prepare_redeem(chain, wallet, &report.pass)?;
                let after = After::Scanned {
                    ticket: report
                        .ticket
                        .as_ref()
                        .map_or_else(String::new, |t| t.name.clone()),
                    holder: report.pass.user,
                };
                (plan, None, after)
            }
        };
        self.modal = Some(Modal::Confirm(Confirm { plan, typed, after }));
        self.info("Review and confirm");
        Ok(())
    }

    fn execute(&mut self, confirm: Confirm) -> Result<()> {
        let me = self.me();
        let mut entry = LogEntry::new(&confirm.plan, &me, &self.cluster, local_now());
        let result = actions::execute(&mut self.chain, &self.wallet, confirm.plan);
        match &result {
            Ok(receipt) => {
                entry.landed(&receipt.signature);
                // Best effort: the Log tab's r fills in what isn't served yet
                if let Ok(Some(details)) = self.chain.transaction(&receipt.signature, &me) {
                    entry.add_details(details);
                }
            }
            Err(error) => entry.failed(error, describe(error)),
        }
        let logged = self.log.push(entry);
        let receipt = result?;
        if let After::Scanned { ticket, holder } = confirm.after {
            self.scans.insert(
                0,
                ScanRecord {
                    time: self.now(),
                    ticket,
                    holder,
                    signature: receipt.signature,
                },
            );
            self.report = None;
        }
        let mut confirmed = format!("{} confirmed · {}", receipt.title, receipt.signature);
        if let Err(error) = logged {
            confirmed += &format!(" (not saved to the log: {error:#})");
        }
        // The transaction landed: a failed refresh mustn't hide its signature
        match self.refresh_and_select(receipt.created) {
            Ok(()) => self.done(confirmed),
            Err(error) => self.error(format!(
                "{confirmed} (refresh failed: {}; press r)",
                describe(&error)
            )),
        }
        Ok(())
    }

    /// Reloads, then selects what was just created (a new event, or a new ticket).
    fn refresh_and_select(&mut self, created: Option<Pubkey>) -> Result<()> {
        self.refresh()?;
        let Some(created) = created else {
            return Ok(());
        };
        if let Some(index) = self
            .visible_events()
            .iter()
            .position(|e| e.address == created)
        {
            self.event_index = index;
            self.load_tickets()?;
        }
        if let Some(index) = self
            .visible_tickets()
            .iter()
            .position(|t| t.asset == created)
        {
            self.ticket_index = index;
            self.pane = Pane::Tickets;
        }
        Ok(())
    }

    fn issue_pass(&mut self, asset: &Pubkey) -> Result<()> {
        let (pass, _, ticket) = actions::issue_pass(&self.chain, &self.wallet, asset)?;
        let now = self.chain.now()?;
        self.clock_offset = now - local_now();
        self.modal = Some(Modal::Pass(PassView {
            pass,
            asset: *asset,
            ticket: ticket.name,
            as_text: false,
        }));
        self.done("Pass ready: show it at the door");
        Ok(())
    }
}

/// Search on the name (every word, any order), or the event ID (with or without `#`).
fn matches_search(event: &EventInfo, search: &str) -> bool {
    let query = search.trim();
    let id = query.trim_start_matches('#');
    query.is_empty()
        || format::matches_words(query, &event.name)
        || (!id.is_empty() && event.config.event_id.to_string().contains(id))
}

fn parse_count(input: &str) -> Result<u8> {
    input
        .parse::<u8>()
        .ok()
        .filter(|&count| count >= 1)
        .ok_or_else(|| anyhow::anyhow!("Tickets: enter a whole number, at least 1"))
}

fn mint_form(event: &EventInfo, minted: u8) -> Form {
    let most = actions::mintable(event, minted);
    let each = event.config.ticket_price + frost_pass::constants::MINT_FEE;
    Form::new(
        FormKind::Mint {
            event: event.address,
        },
        format!("Mint · {}", event.name),
        vec![Field::new(
            "Tickets",
            "1",
            format!(
                "1–{most} · {} USDC each (price + mint fee) · all in one transaction",
                format::usdc(each)
            ),
        )],
    )
}

fn scanners_form(event: &EventInfo) -> Form {
    let current: Vec<String> = event
        .config
        .scanners
        .iter()
        .map(|s| s.to_string())
        .collect();
    Form::new(
        FormKind::Scanners {
            event: event.address,
        },
        format!("Scanners · {}", event.name),
        vec![Field::new(
            "Scanners",
            current.join(", "),
            format!("Replaces the whole list: 1–{MAX_SCANNERS} addresses, no duplicates"),
        )],
    )
}

fn list_form(event: &EventInfo, ticket: &TicketInfo) -> Form {
    let cap = u64::try_from(checks::resale_cap(&event.config)).unwrap_or(u64::MAX);
    Form::new(
        FormKind::List {
            asset: ticket.asset,
        },
        format!("List {}", ticket.name),
        vec![Field::new(
            "Price",
            format::usdc(cap),
            format!(
                "USDC, 0 to {} (face value + {}); 0 lists it for free",
                format::usdc(cap),
                format::percent(event.config.markup_cap_bps)
            ),
        )],
    )
}

fn buy_form(ticket: &TicketInfo) -> Form {
    let price = ticket.state.list_price;
    Form::new(
        FormKind::Buy {
            asset: ticket.asset,
        },
        format!("Buy {}", ticket.name),
        vec![Field::new(
            "Max price",
            format::usdc(price),
            "USDC: the most you'll pay; the purchase fails if the listing costs more",
        )],
    )
}

fn scan_form() -> Form {
    Form::new(
        FormKind::ScanPass,
        "Scan a pass",
        vec![Field::new(
            "Pass",
            "",
            "Paste the holder's pass (frostpass:1:…) or scan its QR code",
        )],
    )
}
