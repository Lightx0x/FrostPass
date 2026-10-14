//! Drawing. Layout: a header with the wallet, one tab per actor, the events list on the left,
//! the selected event and its tickets (or the door, for scanners) on the right, key hints and a
//! status line at the bottom. The Log tab lists this wallet's transactions instead. Modals draw
//! on top.

use frost_pass::constants::{MAX_MINTS_PER_WALLET, MAX_RESALE, MINT_FEE};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row, Table,
        TableState, Tabs, Wrap,
    },
    Frame,
};

use super::{
    app::{App, Hit, HitTarget, Modal, Pane, StatusKind, Tab, TicketRow},
    form::Form,
    theme,
};
use crate::{
    chain::Chain,
    checks, format,
    history::Fate,
    model::{EventInfo, EventStatus},
    store::{LogEntry, LogStatus},
};

pub fn draw<C: Chain>(frame: &mut Frame, app: &App<C>) {
    let [header, _, tabs, body, keys, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    app.hits.borrow_mut().clear();
    draw_header(frame, header, app);
    draw_tabs(frame, tabs, app);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)]).areas(body);
    match app.tab {
        Tab::Log => draw_log(frame, left, right, app),
        Tab::Scanner => {
            draw_events(frame, left, app);
            draw_door(frame, right, app);
        }
        _ => {
            draw_events(frame, left, app);
            draw_event_and_tickets(frame, right, app);
        }
    }
    draw_keys(frame, keys, app);
    draw_status(frame, status, app);

    match &app.modal {
        Some(Modal::Form(form)) => draw_form(frame, form),
        Some(Modal::Confirm(confirm)) => draw_confirm(frame, confirm),
        Some(Modal::Pass(view)) => draw_pass(frame, view, app.now()),
        Some(Modal::Help) => draw_help(frame),
        None => {}
    }
}

fn block(title: &str, focused: bool) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::border(focused))
        .title(Span::styled(format!(" {title} "), theme::title(focused)))
        .padding(Padding::horizontal(1))
}

fn draw_header<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let usdc = app.usdc.map_or_else(
        || "no USDC account".to_string(),
        |a| format!("{} USDC", format::usdc(a)),
    );
    let mut left = theme::logo(app.animation_ms());
    left.extend([
        Span::raw("  "),
        theme::live_dot(app.animation_ms()),
        Span::styled(format!(" {}  ", app.cluster), theme::muted()),
    ]);
    // The wallet: click it to copy the full address
    let wallet = format::short(&app.me());
    let wallet_x = area.x + left.width() as u16;
    app.hits.borrow_mut().push(pane_hit(
        HitTarget::Wallet,
        Rect::new(wallet_x, area.y, wallet.chars().count() as u16, 1),
    ));
    left.extend([
        Span::styled(wallet, theme::strong()),
        Span::styled("  ", theme::muted()),
        Span::styled(format!("{} SOL", format::sol(app.sol)), theme::text()),
        Span::styled("  ·  ", theme::muted()),
        Span::styled(usdc, theme::text()),
    ]);
    let clock = format::datetime(app.now());
    let [left_area, clock_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(clock.len() as u16 + 1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(
        Paragraph::new(Line::styled(clock, theme::muted()).right_aligned()),
        clock_area,
    );
}

fn draw_tabs<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let titles = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, tab)| Line::from(format!(" {} {} ", i + 1, tab.title())));
    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .style(theme::muted())
        .highlight_style(
            Style::new()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
        .divider(Span::styled("│", Style::new().fg(theme::BORDER)))
        .padding("", "");
    frame.render_widget(tabs, area);
    // Each title is clickable: " 1 Create " then a one-column divider
    let mut x = area.x;
    for (index, tab) in Tab::ALL.iter().enumerate() {
        let width = format!(" {} {} ", index + 1, tab.title()).chars().count() as u16;
        app.hits.borrow_mut().push(pane_hit(
            HitTarget::Tab(index),
            Rect::new(x, area.y, width, 1),
        ));
        x += width + 1;
    }
}

/// A clickable area without rows (a click only focuses it).
fn pane_hit(target: HitTarget, area: Rect) -> Hit {
    Hit {
        target,
        area,
        first_row: 0,
        row_height: 0,
        offset: 0,
    }
}

/// The list's title, with the search box when there is one.
fn search_title<C: Chain>(title: &str, app: &App<C>) -> String {
    if app.searching {
        format!("{title} · search: {}▏", app.search)
    } else if app.search.is_empty() {
        title.to_string()
    } else {
        format!("{title} · search: {} (Esc clears)", app.search)
    }
}

fn draw_events<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let focused = app.pane == Pane::Events && app.modal.is_none();
    app.hits
        .borrow_mut()
        .push(pane_hit(HitTarget::Pane(Pane::Events), area));
    let title = match app.tab {
        Tab::Create => "Your events",
        Tab::Scanner => "Events you scan",
        Tab::Ticket | Tab::Marketplace | Tab::Log => "Events",
    };
    let events = app.visible_events();
    let title = if app.ordering {
        search_title(&format!("{title} · sorting newest first…"), app)
    } else {
        search_title(title, app)
    };
    let block = block(&title, focused || app.searching);
    if events.is_empty() && !app.search.is_empty() {
        frame.render_widget(
            Paragraph::new(format!(
                "No events match \"{}\".\n\nSearch by event name (or ID); Esc clears it.",
                app.search
            ))
            .style(theme::muted())
            .wrap(Wrap { trim: true })
            .block(block),
            area,
        );
        return;
    }
    if events.is_empty() {
        let hint = match app.tab {
            Tab::Create => "No events yet.\n\nPress n to create one.",
            Tab::Scanner => {
                "You aren't a scanner of any event.\n\nAn organizer adds this wallet with s in their Create tab."
            }
            Tab::Ticket | Tab::Marketplace | Tab::Log => "No events on this cluster yet.",
        };
        frame.render_widget(
            Paragraph::new(hint)
                .style(theme::muted())
                .wrap(Wrap { trim: true })
                .block(block),
            area,
        );
        return;
    }
    let now = app.now();
    let width = area.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = events
        .iter()
        .map(|event| {
            let status = event.status(now);
            let name: String = event.name.chars().take(width).collect();
            let timing = if status == EventStatus::OnSale {
                format!(
                    "ID {} · sales end {}",
                    event.config.event_id,
                    format::relative(event.config.sales_end, now)
                )
            } else {
                format!(
                    "ID {} · ends {}",
                    event.config.event_id,
                    format::relative(event.config.event_end, now)
                )
            };
            ListItem::new(vec![
                Line::styled(name, theme::strong()),
                Line::from(vec![
                    Span::styled(status.label(), theme::status(status)),
                    Span::styled(
                        format!(
                            " · {} USDC · {}/{} sold",
                            format::usdc(event.config.ticket_price),
                            event.config.tickets_minted,
                            event.config.ticket_supply,
                        ),
                        theme::muted(),
                    ),
                ]),
                Line::styled(timing, theme::muted()),
                Line::raw(""),
            ])
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.event_index));
    let list = List::new(items)
        .block(block)
        .highlight_style(theme::selected(focused))
        .highlight_symbol("");
    frame.render_stateful_widget(list, area, &mut state);
    // Each event takes four rows
    app.hits.borrow_mut().push(Hit {
        target: HitTarget::Pane(Pane::Events),
        area,
        first_row: area.y + 1,
        row_height: 4,
        offset: state.offset(),
    });
}

fn event_lines<C: Chain>(event: &EventInfo, app: &App<C>) -> Vec<Line<'static>> {
    let now = app.now();
    let config = &event.config;
    let status = event.status(now);
    let row = |label: &str, value: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!("{label:<10}"), theme::muted())];
        spans.extend(value);
        Line::from(spans)
    };
    let t = |text: String| Span::styled(text, theme::text());
    let cap = u64::try_from(checks::resale_cap(config)).unwrap_or(u64::MAX);
    let mut lines = vec![
        row("ID", vec![t(config.event_id.to_string())]),
        row(
            "Status",
            vec![
                Span::styled(status.label().to_string(), theme::status(status)),
                t(format!(
                    " · sales end {} ({})",
                    format::relative(config.sales_end, now),
                    format::datetime(config.sales_end)
                )),
            ],
        ),
        row(
            "Event",
            vec![t(format!(
                "ends {} ({})",
                format::relative(config.event_end, now),
                format::datetime(config.event_end)
            ))],
        ),
        row(
            "Price",
            vec![
                t(format!("{} USDC", format::usdc(config.ticket_price))),
                Span::styled(
                    format!(" + {} USDC mint fee", format::usdc(MINT_FEE)),
                    theme::muted(),
                ),
            ],
        ),
        row(
            "Sold",
            vec![t(format!(
                "{} / {} · {} left",
                config.tickets_minted,
                config.ticket_supply,
                event.tickets_left()
            ))],
        ),
        row(
            "Resale",
            vec![t(format!(
                "cap {} USDC (+{}) · royalty {} · up to {MAX_RESALE} resales",
                format::usdc(cap),
                format::percent(config.markup_cap_bps),
                format::percent(config.royalty_bps)
            ))],
        ),
        row(
            "Scanners",
            vec![t(config
                .scanners
                .iter()
                .map(format::short)
                .collect::<Vec<_>>()
                .join(", "))],
        ),
    ];
    let me = app.me();
    let role = match app.tab {
        Tab::Create => format!("organizer · {} tickets out", app.tickets.len()),
        Tab::Ticket => format!("you've minted {} of {MAX_MINTS_PER_WALLET}", app.mints),
        Tab::Marketplace => format!(
            "{} listed",
            app.tickets
                .iter()
                .filter(|t| t.state.seller.is_some())
                .count()
        ),
        Tab::Scanner => {
            if config.scanners.contains(&me) {
                "you're on the scanner list".into()
            } else {
                "you aren't a scanner here".into()
            }
        }
        Tab::Log => String::new(),
    };
    lines.push(row("You", vec![Span::styled(role, theme::accent())]));
    lines
}

fn draw_event_and_tickets<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let [details, tickets] =
        Layout::vertical([Constraint::Length(10), Constraint::Min(4)]).areas(area);
    let Some(event) = app.selected_event() else {
        frame.render_widget(block("Event", false), details);
        frame.render_widget(block("Tickets", false), tickets);
        return;
    };
    frame.render_widget(
        Paragraph::new(event_lines(event, app)).block(block(&event.name, false)),
        details,
    );

    let focused = app.pane == Pane::Tickets && app.modal.is_none();
    let visible = app.visible_tickets();
    let mut title = match app.tab {
        Tab::Create => format!("All tickets ({})", visible.len()),
        Tab::Ticket => format!("Your tickets ({})", visible.len()),
        _ => format!("Listings and your tickets ({})", visible.len()),
    };
    if app.history_loading() {
        title += " · loading history…";
    }
    let block = block(&title, focused);
    app.hits
        .borrow_mut()
        .push(pane_hit(HitTarget::Pane(Pane::Tickets), tickets));
    if visible.is_empty() {
        let hint = match app.tab {
            Tab::Create => "No tickets minted yet.",
            Tab::Ticket => "You have no tickets for this event.\n\nPress m to mint one.",
            _ => "Nothing is listed right now, and you hold no tickets here to sell.",
        };
        frame.render_widget(
            Paragraph::new(hint)
                .style(theme::muted())
                .wrap(Wrap { trim: true })
                .block(block),
            tickets,
        );
        return;
    }
    let me = app.me();
    let rows: Vec<Row> = visible
        .iter()
        .map(|ticket| {
            let number = ticket
                .number
                .map_or_else(|| "?".to_string(), |n| n.to_string());
            let owner = match ticket.owner {
                Some(owner) if owner == me => Span::styled("you", theme::accent()),
                Some(owner) => Span::styled(format::short(&owner), theme::text()),
                None => Span::styled("—", theme::muted()),
            };
            let (status, style) = ticket_status(ticket, &me);
            let dim = |style: Style| {
                if ticket.live.is_some() {
                    style
                } else {
                    style.add_modifier(Modifier::DIM)
                }
            };
            Row::new(vec![
                Cell::from(Span::styled(format!("#{number}"), dim(theme::strong()))),
                Cell::from(owner),
                Cell::from(Span::styled(status, style)),
                Cell::from(Span::styled(
                    format!("{}/{MAX_RESALE}", ticket.resales()),
                    dim(theme::text()),
                )),
                Cell::from(Span::styled(format::short(&ticket.asset), theme::muted())),
            ])
        })
        .collect();
    let tickets = if app.tab == Tab::Create {
        let [tickets, log] =
            Layout::vertical([Constraint::Min(6), Constraint::Percentage(40)]).areas(tickets);
        draw_door_log(frame, log, app);
        app.hits
            .borrow_mut()
            .push(pane_hit(HitTarget::Pane(Pane::Tickets), tickets));
        tickets
    } else {
        tickets
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(11),
            Constraint::Min(30),
            Constraint::Length(8),
            Constraint::Length(12),
        ],
    )
    .header(
        Row::new(["Ticket", "Owner", "Status", "Resales", "Asset"])
            .style(theme::muted().add_modifier(Modifier::BOLD))
            .bottom_margin(1),
    )
    .row_highlight_style(theme::selected(focused))
    .block(block);
    let mut state = TableState::default().with_selected(Some(app.ticket_index));
    frame.render_stateful_widget(table, tickets, &mut state);
    // Below the border, the header and its margin
    app.hits.borrow_mut().push(Hit {
        target: HitTarget::Pane(Pane::Tickets),
        area: tickets,
        first_row: tickets.y + 3,
        row_height: 1,
        offset: state.offset(),
    });
}

/// `"10-12 07:20"`: a short UTC time for tables.
fn short_time(at: Option<i64>) -> String {
    at.map_or_else(
        || "time unknown".to_string(),
        |at| format::datetime(at)[5..16].to_string(),
    )
}

/// A ticket's state for the tables: listed, held, or how it ended (when and by whom).
fn ticket_status(row: &TicketRow, me: &anchor_lang::prelude::Pubkey) -> (String, Style) {
    let who = |key: &anchor_lang::prelude::Pubkey| {
        if key == me {
            "you".to_string()
        } else {
            format::short(key)
        }
    };
    match (&row.live, row.fate()) {
        (Some(ticket), _) => match ticket.listing() {
            Some((_, price)) => (
                format!("Listed · {} USDC", format::usdc(price)),
                theme::ok(),
            ),
            None => ("Held".to_string(), theme::text()),
        },
        (None, Fate::Redeemed { scanner, at, .. }) => (
            format!("Redeemed {} · by {}", short_time(at), who(&scanner)),
            theme::accent(),
        ),
        (None, Fate::Refunded { at }) => (format!("Refunded {}", short_time(at)), theme::warn()),
        (None, Fate::Closed { at }) => (format!("Closed {}", short_time(at)), theme::muted()),
        (None, Fate::Live) => ("Gone".to_string(), theme::muted()),
    }
}

fn draw_door<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let [verdict, log] =
        Layout::vertical([Constraint::Min(14), Constraint::Percentage(40)]).areas(area);
    let focused = app.modal.is_none() && app.pane != Pane::DoorLog;

    let mut lines: Vec<Line> = Vec::new();
    match &app.report {
        None => {
            if let Some(event) = app.selected_event() {
                lines.extend(event_lines(event, app));
                lines.push(Line::raw(""));
            }
            lines.push(Line::from(vec![
                Span::styled("Paste a pass here", theme::strong()),
                Span::styled(" (or press ", theme::muted()),
                Span::styled("v", theme::key()),
                Span::styled(
                    ") to check it. Passes are signed by the ticket's holder and last 60s.",
                    theme::muted(),
                ),
            ]));
        }
        Some(report) => {
            let (word, color) = if report.admits() {
                (" ADMIT ", theme::OK)
            } else {
                (" REFUSE ", theme::DANGER)
            };
            lines.push(Line::from(vec![
                Span::styled(
                    word,
                    Style::new()
                        .bg(color)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    report
                        .ticket
                        .as_ref()
                        .map_or_else(String::new, |t| format!("  {}", t.name)),
                    theme::strong(),
                ),
            ]));
            lines.push(Line::raw(""));
            for check in &report.checks {
                let (mark, style, detail) = match &check.result {
                    Ok(detail) => ("✓", theme::ok(), detail.clone()),
                    Err(detail) => ("✗", theme::danger(), detail.clone()),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{mark} "), style.add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{:<20}", check.label), theme::text()),
                    Span::styled(
                        detail,
                        if check.result.is_ok() {
                            theme::muted()
                        } else {
                            style
                        },
                    ),
                ]));
            }
            lines.push(Line::raw(""));
            lines.push(if report.admits() {
                Line::from(vec![
                    Span::styled("Enter", theme::key()),
                    Span::styled(" redeem (burns the ticket) · ", theme::muted()),
                    Span::styled("Esc", theme::key()),
                    Span::styled(" clear", theme::muted()),
                ])
            } else {
                Line::from(vec![
                    Span::styled("Esc", theme::key()),
                    Span::styled(" clear · ", theme::muted()),
                    Span::styled("v", theme::key()),
                    Span::styled(" scan another pass", theme::muted()),
                ])
            });
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block("Door", focused)),
        verdict,
    );
    draw_door_log(frame, log, app);
}

/// The selected event's door log: every redeem, by every scanner, newest first. Scanners see it
/// under the door; organizers under their tickets.
fn draw_door_log<C: Chain>(frame: &mut Frame, log: Rect, app: &App<C>) {
    let redeems = app.event_redeems();
    let loading = app.history_loading();
    let focused = app.pane == Pane::DoorLog && app.modal.is_none();
    let me = app.me();
    let who = |key: Option<anchor_lang::prelude::Pubkey>| match key {
        Some(key) if key == me => Span::styled(format!("{:<12}", "you"), theme::accent()),
        Some(key) => Span::styled(format!("{:<12}", format::short(&key)), theme::text()),
        None => Span::styled(format!("{:<12}", "—"), theme::muted()),
    };
    let mut title = format!(
        "Door log · {} redeemed ({}) · newest first{}",
        app.selected_event().map_or("", |e| e.name.as_str()),
        redeems.len(),
        if loading { " · loading…" } else { "" }
    );
    if !focused && redeems.len() > 1 {
        title += " · → or click to scroll";
    }
    let block = block(&title, focused);
    let inner = block.inner(log);
    frame.render_widget(block, log);
    let mut hit = Hit {
        target: HitTarget::Pane(Pane::DoorLog),
        area: log,
        first_row: 0,
        row_height: 0,
        offset: 0,
    };
    if redeems.is_empty() {
        frame.render_widget(
            Paragraph::new(if loading {
                "Loading the door log…"
            } else if app.selected_event().is_some() {
                "No tickets redeemed at this event yet."
            } else {
                "Select an event to see its door log."
            })
            .style(theme::muted()),
            inner,
        );
        app.hits.borrow_mut().push(hit);
        return;
    }
    // A fixed header, and the redeems scrolling under it
    let [header, rows] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(
                "{:<13}{:<8}{:<12}{:<12}{}",
                "Time (UTC)", "Ticket", "Scanner", "Holder", "Signature"
            ),
            theme::muted().add_modifier(Modifier::BOLD),
        )),
        header,
    );
    let items: Vec<ListItem> = redeems
        .iter()
        .map(|redeem| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<13}", short_time(redeem.at)), theme::muted()),
                Span::styled(
                    format!(
                        "{:<8}",
                        redeem
                            .number
                            .map_or_else(|| "?".to_string(), |n| format!("#{n}"))
                    ),
                    theme::strong(),
                ),
                who(redeem.scanner),
                who(redeem.holder),
                Span::styled(
                    format!("{}…", redeem.signature.get(..8).unwrap_or("")),
                    theme::muted(),
                ),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.door_index));
    frame.render_stateful_widget(
        List::new(items).highlight_style(theme::selected(focused)),
        rows,
        &mut state,
    );
    hit.first_row = rows.y;
    hit.row_height = 1;
    hit.offset = state.offset();
    app.hits.borrow_mut().push(hit);
}

fn log_status(entry: &LogEntry) -> (&'static str, Style) {
    match entry.status {
        LogStatus::Finalized => ("✓", theme::ok()),
        LogStatus::Confirmed if entry.slot.is_some() => ("✓", theme::warn()),
        LogStatus::Confirmed => ("…", theme::warn()),
        LogStatus::Failed => ("✗", theme::danger()),
    }
}

/// The explorer page of a transaction, for the clusters the explorer knows.
fn explorer_url(signature: &str, cluster: &str) -> String {
    let query = match cluster {
        "devnet" | "testnet" => format!("?cluster={cluster}"),
        "localnet" => "?cluster=custom&customUrl=http%3A%2F%2F127.0.0.1%3A8899".into(),
        url if url.contains("mainnet") => String::new(),
        url => format!("?cluster=custom&customUrl={url}"),
    };
    format!("https://explorer.solana.com/tx/{signature}{query}")
}

fn signed_usdc(amount: i64) -> String {
    let sign = if amount < 0 { "−" } else { "+" };
    format!("{sign}{} USDC", format::usdc(amount.unsigned_abs()))
}

fn signed_sol(lamports: i64) -> String {
    let sign = if lamports < 0 { "−" } else { "+" };
    format!("{sign}{} SOL", format::sol_exact(lamports.unsigned_abs()))
}

fn draw_log<C: Chain>(frame: &mut Frame, left: Rect, right: Rect, app: &App<C>) {
    let entries = app.visible_log();
    let title = search_title(&format!("Transactions ({})", entries.len()), app);
    let list_focused = app.pane == Pane::Events && app.modal.is_none();
    let details_focused = app.pane == Pane::Tickets && app.modal.is_none();
    let list_block = block(&title, list_focused);
    app.hits.borrow_mut().extend([
        pane_hit(HitTarget::Pane(Pane::Events), left),
        pane_hit(HitTarget::Pane(Pane::Tickets), right),
    ]);
    if entries.is_empty() {
        let hint = if app.search.is_empty() {
            match app.log.path() {
                Some(path) => format!(
                    "No transactions yet.\n\nEvery transaction this wallet sends from the app is saved here and in\n{}",
                    path.display()
                ),
                None => "No transactions yet.".to_string(),
            }
        } else {
            format!(
                "No transactions match \"{}\".\n\nSearch by event name, action, event ID or signature; Esc clears it.",
                app.search
            )
        };
        frame.render_widget(
            Paragraph::new(hint)
                .style(theme::muted())
                .wrap(Wrap { trim: true })
                .block(list_block),
            left,
        );
        frame.render_widget(block("Transaction", false), right);
        return;
    }

    let items: Vec<ListItem> = entries
        .iter()
        .map(|entry| {
            let (mark, style) = log_status(entry);
            let event = entry
                .details
                .iter()
                .find(|(label, _)| label == "Event" || label == "Name")
                .map_or_else(String::new, |(_, value)| value.clone());
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(format!("{mark} "), style.add_modifier(Modifier::BOLD)),
                    Span::styled(entry.action.clone(), theme::strong()),
                ]),
                Line::styled(
                    format!(
                        "  {} · {}",
                        format::datetime(entry.block_time.unwrap_or(entry.sent_at)),
                        entry.status.label()
                    ),
                    theme::muted(),
                ),
                Line::styled(format!("  {event}"), theme::muted()),
                Line::raw(""),
            ])
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.log_index));
    frame.render_stateful_widget(
        List::new(items)
            .block(list_block)
            .highlight_style(theme::selected(list_focused)),
        left,
        &mut state,
    );
    app.hits.borrow_mut().push(Hit {
        target: HitTarget::Pane(Pane::Events),
        area: left,
        first_row: left.y + 1,
        row_height: 4,
        offset: state.offset(),
    });

    let Some(entry) = app.selected_log() else {
        return;
    };
    // Long values (the explorer link, notes, logs) wrap under themselves
    let width = right.width.saturating_sub(4).max(1);
    let row = |label: &str, value: String, style: Style| {
        hanging_lines(
            vec![Span::styled(format!("{label:<14}"), theme::muted())],
            &value,
            style,
            width,
        )
    };
    let missing = || "not served yet: press r".to_string();
    let (_, status_style) = log_status(entry);
    let status = match (entry.status, entry.slot) {
        (LogStatus::Failed, _) => "Failed: nothing changed on chain".to_string(),
        (LogStatus::Finalized, _) => "Finalized: rooted, can't be rolled back".to_string(),
        (LogStatus::Confirmed, Some(_)) => {
            "Confirmed by the cluster; press r to check for finality".to_string()
        }
        (LogStatus::Confirmed, None) => {
            "Confirmed; the cluster's details aren't served yet: press r".to_string()
        }
    };
    let mut lines = row("Status", status, status_style);
    if let Some(error) = &entry.error {
        lines.extend(row("Error", error.clone(), theme::danger()));
    }
    if let Some(signature) = &entry.signature {
        lines.extend(row("Signature", signature.clone(), theme::text()));
        lines.extend(row(
            "Explorer",
            explorer_url(signature, &entry.cluster),
            theme::muted(),
        ));
        lines.extend(row(
            "Slot",
            entry.slot.map_or_else(missing, |slot| slot.to_string()),
            theme::text(),
        ));
        lines.extend(row(
            "Block time",
            entry.block_time.map_or_else(missing, format::datetime),
            theme::text(),
        ));
        lines.extend(row(
            "Fee",
            entry.fee.map_or_else(missing, |fee| {
                format!("{} SOL ({fee} lamports)", format::sol_exact(fee))
            }),
            theme::text(),
        ));
        lines.extend(row(
            "Compute units",
            entry
                .compute_units
                .map_or_else(missing, |units| units.to_string()),
            theme::text(),
        ));
        if let (Some(sol), Some(usdc), Some(fee)) = (entry.sol_change, entry.usdc_change, entry.fee)
        {
            // Beyond the fee, SOL moves as rent: paid for new accounts (tickets, records),
            // returned when they're burned or closed
            let why = match sol + fee as i64 {
                0 => " (the fee)",
                rest if rest < 0 => " (fee + rent for new accounts; refunded when they close)",
                _ => " (rent returned, minus the fee)",
            };
            lines.extend(row(
                "SOL change",
                format!("{}{why}", signed_sol(sol)),
                theme::text(),
            ));
            lines.extend(row("USDC change", signed_usdc(usdc), theme::text()));
        }
    }
    lines.extend(row(
        "Sent",
        format!(
            "{} (this computer's clock)",
            format::datetime(entry.sent_at)
        ),
        theme::text(),
    ));
    if let Some(id) = entry.event_id {
        lines.extend(row("Event ID", id.to_string(), theme::text()));
    }
    lines.extend(row(
        "Wallet",
        format!("{} · {}", entry.wallet, entry.cluster),
        theme::text(),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::styled("Instructions", theme::title(false)));
    for (index, name) in entry.instructions.iter().enumerate() {
        lines.push(Line::styled(
            format!("  {}. {name}", index + 1),
            theme::text(),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("What it did", theme::title(false)));
    for (label, value) in &entry.details {
        lines.extend(row(&format!("  {label}"), value.clone(), theme::text()));
    }
    if !entry.logs.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("Program logs ({})", entry.logs.len()),
            theme::title(false),
        ));
        for log in &entry.logs {
            lines.extend(hanging_lines(
                vec![Span::raw("  ")],
                log,
                theme::muted(),
                width,
            ));
        }
    }
    // Scrollable: borders take 2 rows and 2 columns, the padding 2 more columns
    let height = right.height.saturating_sub(2);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = paragraph.line_count(right.width.saturating_sub(4).max(1)) as u16;
    let max = total.saturating_sub(height);
    app.log_scroll_max.set(max);
    app.log_page.set(height.max(1));
    let scroll = app.log_scroll.min(max);
    let title = if max == 0 {
        entry.action.clone()
    } else {
        let hint = if details_focused {
            "↑↓ PgUp PgDn scroll"
        } else {
            "→ to scroll"
        };
        format!(
            "{} · lines {}–{} of {total} · {hint}",
            entry.action,
            scroll + 1,
            (scroll + height).min(total)
        )
    };
    frame.render_widget(
        paragraph
            .scroll((scroll, 0))
            .block(block(&title, details_focused)),
        right,
    );
}

fn draw_keys<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let actions: &[(&str, &str)] = match app.tab {
        Tab::Create => &[
            ("n", "new event"),
            ("s", "scanners"),
            ("c", "cancel event"),
            ("f", "refund"),
        ],
        Tab::Ticket => &[("m", "mint"), ("p", "door pass"), ("x", "close")],
        Tab::Marketplace => &[("b", "buy"), ("l", "list"), ("d", "delist")],
        Tab::Scanner => &[("v", "scan pass"), ("Enter", "redeem"), ("Esc", "clear")],
        Tab::Log => &[
            ("r", "check confirmations"),
            ("c", "copy signature"),
            ("→ PgUp PgDn", "scroll details"),
        ],
    };
    let common: &[(&str, &str)] = &[
        ("/", "search"),
        ("Tab", "actor"),
        ("←→", "pane"),
        ("↑↓", "select"),
        ("r", "refresh"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = Vec::new();
    for (key, label) in actions {
        spans.push(Span::styled(format!(" {key}"), theme::key()));
        spans.push(Span::styled(format!(" {label} "), theme::text()));
    }
    spans.push(Span::styled("  │ ", Style::new().fg(theme::BORDER)));
    for (key, label) in common {
        spans.push(Span::styled(
            format!(" {key}"),
            theme::muted().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}"), theme::muted()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_status<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let (mark, style) = match app.status.kind {
        StatusKind::Info => (" ", theme::muted()),
        StatusKind::Busy => ("…", theme::warn()),
        StatusKind::Ok => ("✓", theme::ok()),
        StatusKind::Error => ("✗", theme::danger()),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {mark} "), style.add_modifier(Modifier::BOLD)),
            Span::styled(app.status.text.clone(), style),
        ])),
        area,
    );
}

// ---------------------------------------------------------------------------
// Modals
// ---------------------------------------------------------------------------

fn modal_area(frame: &Frame, width: u16, height: u16) -> Rect {
    let area = frame.area();
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn modal_block(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title(Span::styled(format!(" {title} "), theme::title(true)))
        .padding(Padding::new(2, 2, 1, 0))
}

/// Borders (2) and horizontal padding (2 + 2) around a modal's text.
const MODAL_CHROME_WIDTH: u16 = 6;
/// Borders (2) and the top padding (1).
const MODAL_CHROME_HEIGHT: u16 = 3;

/// Draws a modal sized to its wrapped content, with the key hints pinned to the bottom so long
/// values (a pasted pass, a scanner list) never push them out of view. When the terminal is too
/// short, the body is cut off rather than the hints.
fn draw_modal(frame: &mut Frame, title: &str, body: Vec<Line>, keys: &[(&str, &str)], width: u16) {
    let width = width.min(frame.area().width);
    let text_width = modal_text_width(frame, width);
    let body = Paragraph::new(body).wrap(Wrap { trim: false });
    let footer = Paragraph::new(footer(keys)).wrap(Wrap { trim: false });
    let body_height = body.line_count(text_width) as u16;
    let footer_height = footer.line_count(text_width) as u16;
    let height = body_height + 1 + footer_height + MODAL_CHROME_HEIGHT;

    let area = modal_area(frame, width, height);
    let block = modal_block(title);
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let [body_area, _, footer_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(footer_height),
    ])
    .areas(inner);
    frame.render_widget(body, body_area);
    frame.render_widget(footer, footer_area);
}

/// The width of a modal's text area on this terminal.
fn modal_text_width(frame: &Frame, width: u16) -> u16 {
    width
        .min(frame.area().width)
        .saturating_sub(MODAL_CHROME_WIDTH)
        .max(1)
}

/// A `label  value` row whose value wraps under itself instead of under the label. The last
/// column is left free so a cursor after the value doesn't spill onto a new row.
fn hanging_lines(
    lead: Vec<Span<'static>>,
    value: &str,
    style: Style,
    width: u16,
) -> Vec<Line<'static>> {
    let indent: usize = lead.iter().map(Span::width).sum();
    let rows = wrap_words(value, (width as usize).saturating_sub(indent + 1).max(1));
    let mut lines = Vec::with_capacity(rows.len());
    let mut lead = Some(lead);
    for row in rows {
        let mut spans = lead
            .take()
            .unwrap_or_else(|| vec![Span::raw(" ".repeat(indent))]);
        spans.push(Span::styled(row, style));
        lines.push(Line::from(spans));
    }
    lines
}

/// Greedy word wrap to `width` columns; words longer than a row are split. Always returns at
/// least one row.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    for word in text.split_inclusive(' ') {
        // The row keeps its trailing space, so this is the width with the word appended
        let row = rows.last().unwrap();
        if !row.is_empty() && row.chars().count() + word.trim_end().chars().count() > width {
            rows.push(String::new());
        }
        for c in word.chars() {
            let row = rows.last_mut().unwrap();
            if row.chars().count() >= width && c != ' ' {
                rows.push(String::new());
            }
            rows.last_mut().unwrap().push(c);
        }
    }
    for row in &mut rows {
        let trimmed = row.trim_end().len();
        row.truncate(trimmed);
    }
    rows
}

fn footer(keys: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (key, label) in keys {
        spans.push(Span::styled(key.to_string(), theme::key()));
        spans.push(Span::styled(format!(" {label}   "), theme::muted()));
    }
    Line::from(spans)
}

/// The text cursor in form fields.
const CURSOR: char = '▏';

/// Splits the span holding [`CURSOR`] so the cursor shows in the accent.
fn cursor_in_accent(line: Line<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 2);
    for span in line.spans {
        match span.content.split_once(CURSOR) {
            Some((before, after)) => {
                let (before, after) = (before.to_string(), after.to_string());
                spans.push(Span::styled(before, span.style));
                spans.push(Span::styled(CURSOR.to_string(), theme::accent()));
                spans.push(Span::styled(after, span.style));
            }
            None => spans.push(span),
        }
    }
    Line::from(spans)
}

const FORM_WIDTH: u16 = 84;

fn draw_form(frame: &mut Frame, form: &Form) {
    let width = modal_text_width(frame, FORM_WIDTH);
    let mut lines = Vec::new();
    let label_width = form.fields.iter().map(|f| f.label.len()).max().unwrap_or(0) + 2;
    for (index, field) in form.fields.iter().enumerate() {
        let focused = index == form.focus;
        let lead = vec![
            Span::styled(if focused { "▌ " } else { "  " }, theme::accent()),
            Span::styled(
                format!("{:<label_width$}", field.label),
                if focused {
                    theme::title(true)
                } else {
                    theme::muted()
                },
            ),
        ];
        let mut rows = if focused {
            // The cursor sits where typing goes; wrap it with the text, then colour it
            let mut value = field.value.clone();
            let at = value
                .char_indices()
                .nth(field.cursor)
                .map_or(value.len(), |(byte, _)| byte);
            value.insert(at, CURSOR);
            hanging_lines(lead, &value, theme::strong(), width)
                .into_iter()
                .map(cursor_in_accent)
                .collect()
        } else {
            hanging_lines(lead, &field.value, theme::strong(), width)
        };
        if focused {
            rows.extend(hanging_lines(
                vec![Span::raw(" ".repeat(label_width + 2))],
                &field.hint,
                theme::muted(),
                width,
            ));
        }
        lines.extend(rows);
    }
    if let Some(error) = &form.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            error.clone(),
            theme::danger().add_modifier(Modifier::BOLD),
        ));
    }
    draw_modal(
        frame,
        &form.title,
        lines,
        &[
            ("Tab/↓", "next"),
            ("←→ Home End", "move"),
            ("Ctrl+⌫", "delete word"),
            ("Ctrl+U", "clear"),
            ("Enter", "next / submit"),
            ("Ctrl+S", "submit"),
            ("Esc", "cancel"),
        ],
        FORM_WIDTH,
    );
}

const CONFIRM_WIDTH: u16 = 88;

fn draw_confirm(frame: &mut Frame, confirm: &super::app::Confirm) {
    let plan = &confirm.plan;
    let width = modal_text_width(frame, CONFIRM_WIDTH);
    let mut lines: Vec<Line> = plan
        .lines
        .iter()
        .flat_map(|(label, value)| {
            hanging_lines(
                vec![Span::styled(format!("{label:<13}"), theme::muted())],
                value,
                theme::text(),
                width,
            )
        })
        .collect();
    if let Some(warning) = &plan.warning {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            warning.clone(),
            theme::danger().add_modifier(Modifier::BOLD),
        ));
    }
    let keys: &[(&str, &str)] = match &confirm.typed {
        Some((word, typed)) => {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("Type ", theme::muted()),
                Span::styled(*word, theme::strong()),
                Span::styled(" to confirm: ", theme::muted()),
                Span::styled(typed.clone(), theme::strong()),
                Span::styled("▏", theme::accent()),
            ]));
            &[("Enter", "confirm"), ("Esc", "back")]
        }
        None => &[("Enter / y", "confirm and send"), ("Esc / n", "back")],
    };
    draw_modal(frame, &plan.title, lines, keys, CONFIRM_WIDTH);
}

/// The door pass as a QR code: compact sextants by default (27×18 cells, plenty for a phone),
/// or full-size half blocks with z (twice as big each way, for a far or dim scanner). Each
/// goes in the normal popup if it fits, else a tight frame with just the essentials; a large
/// code that doesn't fit falls back to the compact one, and text shows only when nothing fits.
fn draw_pass(frame: &mut Frame, view: &super::app::PassView, now: i64) {
    let expiry = view.pass.expiry;
    let toggle = if view.as_text { "show QR" } else { "show text" };
    let zoom = if view.large { "smaller" } else { "bigger" };
    let keys = [
        ("c", "copy"),
        ("t", toggle),
        ("z", zoom),
        ("r", "new pass"),
        ("Esc", "close"),
    ];
    // The tight frame's keys, short enough for two rows under the compact code
    let tight_keys = [
        ("c", "copy"),
        ("t", "text"),
        ("z", zoom),
        ("r", "new"),
        ("Esc", "close"),
    ];
    let validity = if now <= expiry {
        Line::styled(
            format!("Valid for {}s · signed by your wallet", expiry - now),
            theme::ok(),
        )
    } else {
        Line::styled(
            "Expired: press r for a new pass",
            theme::danger().add_modifier(Modifier::BOLD),
        )
    };
    let area = frame.area();
    let footer_rows = |keys: &[(&str, &str)], width: u16| {
        Paragraph::new(footer(keys))
            .wrap(Wrap { trim: false })
            .line_count(width.max(1)) as u16
    };

    if !view.as_text {
        let sizes = if view.large {
            vec![view.pass.qr_rows(), view.pass.qr_rows_compact()]
        } else {
            vec![view.pass.qr_rows_compact()]
        };
        for qr in sizes.iter().filter_map(|qr| qr.as_ref().ok()) {
            let width = qr.first().map_or(0, |row| row.chars().count()) as u16;
            let height = qr.len() as u16;
            // The normal popup: padding, the ticket and its validity above the code
            let popup_width = (width + MODAL_CHROME_WIDTH).max(64).min(area.width);
            let text_width = popup_width.saturating_sub(MODAL_CHROME_WIDTH);
            if width <= text_width
                && 3 + height + 1 + footer_rows(&keys, text_width) + MODAL_CHROME_HEIGHT
                    <= area.height
            {
                let mut lines = vec![
                    Line::styled(view.ticket.clone(), theme::strong()),
                    validity,
                    Line::raw(""),
                ];
                lines.extend(
                    qr.iter()
                        .map(|row| Line::styled(row.clone(), qr_style()).centered()),
                );
                draw_modal(frame, "Door pass", lines, &keys, popup_width);
                return;
            }
            // The tight frame: borders, the code, the keys; ticket and time in the title
            let tight_height = height + 2 + footer_rows(&tight_keys, width);
            if width + 2 <= area.width && tight_height <= area.height {
                draw_pass_tight(frame, view, qr, now, &tight_keys, tight_height);
                return;
            }
        }
    }

    // Nothing fits: say so first (in a tiny window it's all that shows), then the text
    let mut lines = Vec::new();
    if !view.as_text {
        lines.push(Line::styled(
            "Too small for the QR code (needs about 30×21): c copies the pass",
            theme::warn(),
        ));
        lines.push(Line::raw(""));
    }
    lines.extend([
        Line::styled(view.ticket.clone(), theme::strong()),
        validity,
        Line::raw(""),
        Line::styled(view.pass.encode(), theme::strong()),
    ]);
    draw_modal(frame, "Door pass", lines, &keys, 64);
}

/// Dark modules on light, whatever the terminal's colours.
fn qr_style() -> Style {
    Style::new().fg(Color::Black).bg(Color::White)
}

fn draw_pass_tight(
    frame: &mut Frame,
    view: &super::app::PassView,
    qr: &[String],
    now: i64,
    keys: &[(&str, &str)],
    height: u16,
) {
    let width = qr.first().map_or(0, |row| row.chars().count()) as u16;
    let number = view.ticket.rsplit_once('#').map_or("pass", |(_, n)| n);
    let (time, style) = if now <= view.pass.expiry {
        (format!("{}s", view.pass.expiry - now), theme::ok())
    } else {
        ("expired: r".to_string(), theme::danger())
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title(Line::from(vec![
            Span::styled(format!(" #{number} · "), theme::title(true)),
            Span::styled(format!("{time} "), style.add_modifier(Modifier::BOLD)),
        ]));
    let area = modal_area(frame, width + 2, height);
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let [code, keys_area] =
        Layout::vertical([Constraint::Length(qr.len() as u16), Constraint::Min(0)]).areas(inner);
    frame.render_widget(
        Paragraph::new(
            qr.iter()
                .map(|row| Line::styled(row.clone(), qr_style()))
                .collect::<Vec<_>>(),
        ),
        code,
    );
    frame.render_widget(
        Paragraph::new(footer(keys)).wrap(Wrap { trim: false }),
        keys_area,
    );
}

fn draw_help(frame: &mut Frame) {
    let section = |title: &str| Line::styled(title.to_string(), theme::title(true));
    let item = |key: &str, text: &str| {
        Line::from(vec![
            Span::styled(format!("  {key:<7}"), theme::key()),
            Span::styled(text.to_string(), theme::text()),
        ])
    };
    let rule = |text: &str| Line::styled(format!("  · {text}"), theme::muted());
    let lines = vec![
        section("Create (tab 1, organizers)"),
        item("n", "Create an event: price, supply, resale cap (≤20%), royalty (≤5%), sales end, event end, scanners"),
        item("s", "Replace the scanner list (1–5 wallets): revoke a lost or compromised scanner"),
        item("c", "Cancel the event (irreversible); then f refunds each holder at face value"),
        rule("Every ticket and its status, and the door log: who redeemed what, when"),
        Line::raw(""),
        section("Ticket (tab 2)"),
        item("m", "Mint 1–3 tickets in one transaction: price to the organizer + 0.25 USDC fee each; 3 per wallet per event"),
        item("p", "Door pass: a QR your wallet signs, valid 60s. Only the current owner can make one"),
        item("x", "Close the ticket after the event ends and get its rent back"),
        Line::raw(""),
        section("Marketplace (tab 3)"),
        item("b", "Buy the selected listing at its price: royalty to the organizer, the rest to the seller"),
        item("l / d", "List one of your tickets (0 up to face value + resale cap) / delist it. It stays in your wallet"),
        Line::raw(""),
        section("Scanner"),
        item("v", "Paste or scan a pass; Enter redeems it (burns the ticket, rents go to the holder)"),
        Line::raw(""),
        section("Log (tab 5)"),
        item("r", "Check confirmations: fills in slot, fee and logs, and marks finalized transactions"),
        item("c", "Copy the selected transaction's signature"),
        item("→ ↑↓", "Scroll the details and program logs (PgUp/PgDn a page, Home/End the ends); ← back to the list"),
        Line::raw(""),
        section("Everywhere"),
        item("/", "Search events by name (or ID); in the Log also by action or signature. Esc clears"),
        Line::raw(""),
        section("Rules the app checks before sending"),
        rule("Tickets are frozen: they move only through FrostPass resales, and resell at most twice"),
        rule("Mint, list and buy close at sales end; redeeming works until the event ends"),
        rule("A cancelled event allows only refunds, delisting and closing"),
        rule("Times follow the cluster clock, not this computer's"),
        rule("Each wallet acts as itself: run with --keypair to act as another wallet"),
    ];
    draw_modal(frame, "FrostPass help", lines, &[("any key", "close")], 104);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_wrap_under_themselves() {
        assert_eq!(wrap_words("", 10), vec![""]);
        assert_eq!(wrap_words("aaa, bbb, ccc", 9), vec!["aaa, bbb,", "ccc"]);
        assert_eq!(wrap_words("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);

        let lines = hanging_lines(
            vec![Span::raw("Label  ")],
            "aaa, bbb, ccc",
            Style::new(),
            17,
        );
        let rows: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(rows, vec!["Label  aaa, bbb,", "       ccc"]);
        assert!(lines.iter().all(|line| line.width() < 17));
    }
}
