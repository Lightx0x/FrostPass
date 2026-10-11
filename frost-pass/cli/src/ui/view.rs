//! Drawing. Layout: a header with the wallet, one tab per actor, the events list on the left,
//! the selected event and its tickets (or the door, for scanners) on the right, key hints and a
//! status line at the bottom. Modals draw on top.

use frost_pass::constants::{MAX_MINTS_PER_WALLET, MAX_RESALE, MINT_FEE};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row, Table,
        TableState, Tabs, Wrap,
    },
    Frame,
};

use super::{
    app::{App, Modal, Pane, StatusKind, Tab},
    form::Form,
    theme,
};
use crate::{
    chain::Chain,
    checks, format,
    model::{EventInfo, EventStatus},
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

    draw_header(frame, header, app);
    draw_tabs(frame, tabs, app);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)]).areas(body);
    draw_events(frame, left, app);
    if app.tab == Tab::Scanner {
        draw_door(frame, right, app);
    } else {
        draw_event_and_tickets(frame, right, app);
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
    let left = Line::from(vec![
        Span::styled(
            " FROSTPASS ",
            Style::new()
                .bg(theme::ACCENT)
                .fg(theme::ON_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", app.cluster), theme::muted()),
        Span::styled("  wallet ", theme::muted()),
        Span::styled(format::short(&app.me()), theme::strong()),
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
}

fn draw_events<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let focused = app.pane == Pane::Events && app.modal.is_none();
    let title = match app.tab {
        Tab::Organizer => "Your events",
        Tab::Scanner => "Events you scan",
        Tab::Minter | Tab::Buyer => "Events",
    };
    let events = app.visible_events();
    let block = block(title, focused);
    if events.is_empty() {
        let hint = match app.tab {
            Tab::Organizer => "No events yet.\n\nPress n to create one.",
            Tab::Scanner => {
                "You aren't a scanner of any event.\n\nAn organizer adds this wallet with s in their Organizer tab."
            }
            Tab::Minter | Tab::Buyer => "No events on this cluster yet.",
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
                    "sales end {}",
                    format::relative(event.config.sales_end, now)
                )
            } else {
                format!("ends {}", format::relative(event.config.event_end, now))
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
        Tab::Organizer => format!("organizer · {} tickets out", app.tickets.len()),
        Tab::Minter => format!("you've minted {} of {MAX_MINTS_PER_WALLET}", app.mints),
        Tab::Buyer => format!(
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
    };
    lines.push(row("You", vec![Span::styled(role, theme::accent())]));
    lines
}

fn draw_event_and_tickets<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let [details, tickets] =
        Layout::vertical([Constraint::Length(9), Constraint::Min(4)]).areas(area);
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
    let title = match app.tab {
        Tab::Organizer => format!("All tickets ({})", visible.len()),
        Tab::Minter => format!("Your tickets ({})", visible.len()),
        _ => format!("Listings and your tickets ({})", visible.len()),
    };
    let block = block(&title, focused);
    if visible.is_empty() {
        let hint = match app.tab {
            Tab::Organizer => "No tickets minted yet.",
            Tab::Minter => "You have no tickets for this event.\n\nPress m to mint one.",
            _ => "Nothing is listed right now, and you hold no tickets here.",
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
            let number = ticket.name.rsplit('#').next().unwrap_or("?");
            let owner = if ticket.owner == me {
                Span::styled("you", theme::accent())
            } else {
                Span::styled(format::short(&ticket.owner), theme::text())
            };
            let listing = match ticket.listing() {
                Some((_, price)) => {
                    Span::styled(format!("{} USDC", format::usdc(price)), theme::ok())
                }
                None => Span::styled("—", theme::muted()),
            };
            Row::new(vec![
                Cell::from(Span::styled(format!("#{number}"), theme::strong())),
                Cell::from(owner),
                Cell::from(listing),
                Cell::from(Span::styled(
                    format!("{}/{MAX_RESALE}", ticket.state.resale_count),
                    theme::text(),
                )),
                Cell::from(Span::styled(format::short(&ticket.asset), theme::muted())),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(11),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Min(10),
        ],
    )
    .header(
        Row::new(["Ticket", "Owner", "Listed at", "Resales", "Asset"])
            .style(theme::muted().add_modifier(Modifier::BOLD))
            .bottom_margin(1),
    )
    .row_highlight_style(theme::selected(focused))
    .block(block);
    let mut state = TableState::default().with_selected(Some(app.ticket_index));
    frame.render_stateful_widget(table, tickets, &mut state);
}

fn draw_door<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let [verdict, log] = Layout::vertical([Constraint::Min(12), Constraint::Length(8)]).areas(area);
    let focused = app.modal.is_none();

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
                (" REFUSE ", theme::ACCENT)
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
                    Err(detail) => ("✗", theme::accent(), detail.clone()),
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

    let items: Vec<ListItem> = if app.scans.is_empty() {
        vec![ListItem::new(Line::styled(
            "No tickets redeemed yet this session.",
            theme::muted(),
        ))]
    } else {
        app.scans
            .iter()
            .map(|scan| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format::datetime(scan.time)[11..16].to_string(),
                        theme::muted(),
                    ),
                    Span::styled(format!("  {:<28}", scan.ticket), theme::text()),
                    Span::styled(format::short(&scan.holder), theme::muted()),
                    Span::styled(
                        format!("  {}…", &scan.signature.to_string()[..8]),
                        theme::muted(),
                    ),
                ]))
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(block(
            &format!("Redeemed this session ({})", app.scans.len()),
            false,
        )),
        log,
    );
}

fn draw_keys<C: Chain>(frame: &mut Frame, area: Rect, app: &App<C>) {
    let actions: &[(&str, &str)] = match app.tab {
        Tab::Organizer => &[
            ("n", "new event"),
            ("s", "scanners"),
            ("c", "cancel event"),
            ("f", "refund"),
        ],
        Tab::Minter => &[
            ("m", "mint"),
            ("l", "list"),
            ("d", "delist"),
            ("p", "door pass"),
            ("x", "close"),
        ],
        Tab::Buyer => &[
            ("b", "buy"),
            ("l", "list"),
            ("d", "delist"),
            ("p", "door pass"),
            ("x", "close"),
        ],
        Tab::Scanner => &[("v", "scan pass"), ("Enter", "redeem"), ("Esc", "clear")],
    };
    let common: &[(&str, &str)] = &[
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
        StatusKind::Error => ("✗", theme::accent()),
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

fn footer(keys: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (key, label) in keys {
        spans.push(Span::styled(key.to_string(), theme::key()));
        spans.push(Span::styled(format!(" {label}   "), theme::muted()));
    }
    Line::from(spans)
}

fn draw_form(frame: &mut Frame, form: &Form) {
    let mut lines = Vec::new();
    let label_width = form.fields.iter().map(|f| f.label.len()).max().unwrap_or(0) + 2;
    for (index, field) in form.fields.iter().enumerate() {
        let focused = index == form.focus;
        let mut line = vec![
            Span::styled(if focused { "▌ " } else { "  " }, theme::accent()),
            Span::styled(
                format!("{:<label_width$}", field.label),
                if focused {
                    theme::title(true)
                } else {
                    theme::muted()
                },
            ),
            Span::styled(field.value.clone(), theme::strong()),
        ];
        if focused {
            line.push(Span::styled("▏", theme::accent()));
        }
        lines.push(Line::from(line));
        if focused {
            lines.push(Line::styled(
                format!("  {:<label_width$}{}", "", field.hint),
                theme::muted(),
            ));
        }
    }
    if let Some(error) = &form.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            error.clone(),
            theme::accent().add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(footer(&[
        ("Tab/↓", "next"),
        ("Enter", "next / submit"),
        ("Ctrl+S", "submit"),
        ("Ctrl+U", "clear"),
        ("Esc", "cancel"),
    ]));
    let height = lines.len() as u16 + 4;
    let area = modal_area(frame, 84, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(modal_block(&form.title)),
        area,
    );
}

fn draw_confirm(frame: &mut Frame, confirm: &super::app::Confirm) {
    let plan = &confirm.plan;
    let mut lines: Vec<Line> = plan
        .lines
        .iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!("{label:<13}"), theme::muted()),
                Span::styled(value.clone(), theme::text()),
            ])
        })
        .collect();
    if let Some(warning) = &plan.warning {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            warning.clone(),
            theme::accent().add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::raw(""));
    match &confirm.typed {
        Some((word, typed)) => {
            lines.push(Line::from(vec![
                Span::styled("Type ", theme::muted()),
                Span::styled(*word, theme::strong()),
                Span::styled(" to confirm: ", theme::muted()),
                Span::styled(typed.clone(), theme::strong()),
                Span::styled("▏", theme::accent()),
            ]));
            lines.push(Line::raw(""));
            lines.push(footer(&[("Enter", "confirm"), ("Esc", "back")]));
        }
        None => lines.push(footer(&[
            ("Enter / y", "confirm and send"),
            ("Esc / n", "back"),
        ])),
    }
    let height = lines.len() as u16 + 4;
    let area = modal_area(frame, 88, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(modal_block(&plan.title)),
        area,
    );
}

fn draw_pass(frame: &mut Frame, view: &super::app::PassView, now: i64) {
    let qr = view.pass.qr_rows().unwrap_or_default();
    let qr_width = qr.first().map_or(0, |row| row.chars().count()) as u16;
    let expiry = view.pass.expiry;
    let text = view.pass.encode();

    let mut lines = vec![
        Line::styled(view.ticket.clone(), theme::strong()),
        if now <= expiry {
            Line::styled(
                format!("Valid for {}s · signed by your wallet", expiry - now),
                theme::ok(),
            )
        } else {
            Line::styled(
                "Expired: press r for a new pass",
                theme::accent().add_modifier(Modifier::BOLD),
            )
        },
        Line::raw(""),
    ];
    let area = frame.area();
    let fits = qr_width + 8 <= area.width && qr.len() as u16 + 12 <= area.height;
    if fits && !view.as_text {
        let qr_style = Style::new().fg(Color::Black).bg(Color::White);
        lines.extend(qr.into_iter().map(|row| Line::styled(row, qr_style)));
    } else {
        if !fits {
            lines.push(Line::styled(
                "Enlarge the terminal to show the QR code; the text works too.",
                theme::warn(),
            ));
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(text, theme::strong()));
    }
    lines.push(Line::raw(""));
    lines.push(footer(&[
        ("c", "copy"),
        ("t", if view.as_text { "show QR" } else { "show text" }),
        ("r", "new pass"),
        ("Esc", "close"),
    ]));

    let width = (qr_width + 8).max(64);
    let height = (lines.len() as u16 + 4).min(area.height);
    let area = modal_area(frame, width, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(modal_block("Door pass")),
        area,
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
        section("Organizer"),
        item("n", "Create an event: price, supply, resale cap (≤20%), royalty (≤5%), sales end, event end, scanners"),
        item("s", "Replace the scanner list (1–5 wallets): revoke a lost or compromised scanner"),
        item("c", "Cancel the event (irreversible); then f refunds each holder at face value"),
        Line::raw(""),
        section("Minter"),
        item("m", "Mint a ticket: ticket price to the organizer + 0.25 USDC fee; 3 per wallet per event"),
        Line::raw(""),
        section("Buyer"),
        item("b", "Buy a listing up to your max price: royalty to the organizer, the rest to the seller"),
        Line::raw(""),
        section("Ticket holders (Minter and Buyer tabs)"),
        item("l / d", "List at 0 up to face value + markup cap / delist. The ticket stays in your wallet"),
        item("p", "Door pass: a QR your wallet signs, valid 60s. Only the current owner can make one"),
        item("x", "Close the ticket after the event ends and get its rent back"),
        Line::raw(""),
        section("Scanner"),
        item("v", "Paste or scan a pass; Enter redeems it (burns the ticket, rents go to the holder)"),
        Line::raw(""),
        section("Rules the app checks before sending"),
        rule("Tickets are frozen: they move only through FrostPass resales, and resell at most twice"),
        rule("Mint, list and buy close at sales end; redeeming works until the event ends"),
        rule("A cancelled event allows only refunds, delisting and closing"),
        rule("Times follow the cluster clock, not this computer's"),
        rule("Each wallet acts as itself: run with --keypair to act as another wallet"),
        Line::raw(""),
        footer(&[("any key", "close")]),
    ];
    let height = lines.len() as u16 + 4;
    let area = modal_area(frame, 104, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(modal_block("FrostPass help")),
        area,
    );
}
