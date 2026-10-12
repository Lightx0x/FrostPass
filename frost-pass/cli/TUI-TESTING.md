# FrostPass TUI: devnet testing guide

How to test `frostpass`, the terminal app (ratatui TUI) for the FrostPass Solana program, against the **live devnet deployment** with seven throwaway wallets: run the test plan below, record what actually happens, and report bugs. Read the whole guide before starting.

## Ground rules

- **Devnet only.** Never point the app at mainnet. Every wallet and all USDC here are worthless test assets.
- **Don't change the on-chain program** (`programs/frost-pass/`). It's deployed; this is a client test.
- **Never print or share keypair files or recovery phrases.** Public addresses are fine.
- Every action in the app shows a confirmation screen first. Read it, and check it matches what you expect, before confirming.

## What FrostPass is

Event ticketing on Solana (Anchor 1.2, Rust). Each ticket is a **Metaplex Core asset** that stays **frozen** in the holder's wallet for its whole life, so it can't be transferred outside the program. Every resale goes through the program, which enforces a price cap, a resale limit and a royalty. At the door, a scanner redeems the ticket (the program burns it) using a short-lived **door pass** the holder's wallet signs.

| | |
|---|---|
| Program ID (devnet) | `8GqPBbxtkA9PbQYmAfb4SrU56tHiYaBh5kPUsHtM2hjk` |
| Payment mint (devnet USDC) | `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU` (6 decimals) |
| Protocol treasury (receives the mint fee) | `9TRRTfqqfeMiACKETs6NUgdAamyxYLggNMbB2S1Jte97` |
| RPC | `https://api.devnet.solana.com` (rate-limited; retry later on 429s) |

### The rules the program enforces (and the app checks before sending)

| Action | Rules |
|---|---|
| Create event | ends 1 hour–1 year from now; sales end in the future and no later than the event end; price > 0; supply > 0; resale cap ≤ 20%; royalty ≤ 5%; name 1–32 bytes; URI 1–200 bytes; 1–5 scanners, no duplicates |
| Mint | not cancelled; before sales end; ≤ 3 mints per wallet per event; not sold out. Cost per ticket: ticket price → organizer, plus a fixed **0.25 USDC** fee → treasury. Up to 3 can be minted in one transaction |
| List | not cancelled; before sales end; only the owner; not already listed; resold < 2 times; price from 0 up to face value × (1 + cap) |
| Delist | only the wallet that listed it; works any time |
| Buy | not your own listing; not cancelled; before sales end; seller still owns it; price ≤ your max. Royalty → organizer, the rest → seller, **no protocol fee** |
| Redeem | scanner is on the event's list; not cancelled; before event end (sales ending does **not** stop entry); pass ≤ 90s validity and not expired; pass signer still owns the ticket. Burns the ticket; the holder gets both rents back |
| Cancel event | organizer only; irreversible |
| Refund | organizer only; cancelled events only; pays face value (not the resale price; the mint fee isn't refunded) to the current owner and burns the ticket |
| Close ticket | owner only; after event end; burns it and returns the rents |

**App-side extras** (not program rules, but the app blocks them so the transaction can't fail): enough USDC, required USDC accounts exist, and **an organizer can't mint or buy tickets of their own event** (their USDC account would be both payer and payee, which Anchor rejects with `ConstraintDuplicateMutableAccount`).

A blocked action shows `✗ Blocked: <reason> [<RuleName>]` in the status line and **sends nothing**. A transaction the chain rejects shows `✗ Transaction failed: rejected by the program: <Error> (<code>)`. A success shows `✓ <Action> confirmed · <signature>`.

## Setup

Run every command from the workspace folder `frost-pass/` (the one with the workspace `Cargo.toml`), unless it says otherwise.

### Build and offline tests

```bash
cargo build -p frost-pass-cli
```

The binary is `target/debug/frostpass`. (A `--release` build also works but takes several minutes: the workspace uses full LTO.)

```bash
cargo test -p frost-pass-cli
```

The CLI tests run every action against the real program in LiteSVM, so they need the program binary at `target/deploy/frost_pass.so`. If it's missing, build it first:

```bash
cargo build-sbf --manifest-path programs/frost-pass/Cargo.toml --sbf-out-dir target/deploy
```

### Wallets

You need seven devnet wallets. Keep them together in one folder (any folder; this guide uses `~/.config/solana/frostpass/`):

| Actor | File | SOL | USDC |
|---|---|---|---|
| Organizer | `organizer.json` | ~1 | none (creating the first event creates its USDC account) |
| Holder | `holder.json` | ~1 | 10+ |
| Buyer | `buyer.json` | ~1 | 10+ |
| Buyer 2 | `buyer2.json` | ~1 | 10+ |
| Scanner | `scanner.json` | ~0.1 | none needed |
| Scanner 2 | `scanner2.json` | ~0.1 | none needed |
| Scanner 3 | `scanner3.json` | ~0.1 | none needed |

Create them. `--silent` keeps the recovery phrase off the screen; these are throwaway devnet wallets, so you don't need it:

```bash
mkdir -p ~/.config/solana/frostpass && for a in organizer holder buyer buyer2 scanner scanner2 scanner3; do solana-keygen new --no-bip39-passphrase --silent -o ~/.config/solana/frostpass/$a.json >/dev/null; done
```

Fund them:

- **SOL:** `solana airdrop -u d 1 <ADDRESS>` per wallet. The devnet faucet rate-limits often; if it refuses, use https://faucet.solana.com, or send SOL from a wallet that already has some (`solana transfer -u d --keypair <FROM>.json --allow-unfunded-recipient <ADDRESS> 0.1`). Scanners only pay transaction fees, so 0.1 SOL is plenty.
- **USDC:** the payment mint is Circle's devnet USDC. Get it at https://faucet.circle.com (pick Solana Devnet) for the holder, buyer and buyer 2.

List every wallet's address and balances (useful before and after each money-moving step):

```bash
for a in organizer holder buyer buyer2 scanner scanner2 scanner3; do k=$(solana-keygen pubkey ~/.config/solana/frostpass/$a.json); echo "$a $k: $(solana balance -u d $k) · USDC: $(spl-token balance -u d --owner $k 4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU 2>/dev/null || echo none)"; done
```

The test plan refers to the scanners' addresses as `<SCANNER>`, `<SCANNER2>` and `<SCANNER3>`; take them from that list.

Confirm a transaction landed: `solana confirm -u d -v <signature>`.

### Testing by hand (one tmux window per actor)

From inside tmux, in `frost-pass/`, open a named window per actor (like `Ctrl+b c`), staggered by a second to avoid RPC 429s on startup:

```bash
B=$PWD/target/debug/frostpass; K=~/.config/solana/frostpass; for a in organizer holder buyer buyer2 scanner scanner2 scanner3; do W=$(tmux new-window -d -P -F '#{window_id}' -n $a "$B --keypair $K/$a.json"); tmux set -w -t $W remain-on-exit on; sleep 1; done; tmux select-window -t organizer
```

Switch with `Ctrl+b w` (pick by name), `Ctrl+b n`/`p`, or `Ctrl+b <number>`. If a window sticks on "Loading events…", press `r`. A window stays open after the app exits so you can read any error. Close them all:

```bash
for a in organizer holder buyer buyer2 scanner scanner2 scanner3; do tmux kill-window -t $a; done
```

**Moving a door pass between windows:** press `c` on the holder's pass screen, switch to the scanner's window, and press `Ctrl+b ]` to paste tmux's buffer. `c` also puts the pass on the system clipboard when the terminal supports OSC 52 or `xclip`, `xsel`, `wl-copy` or `pbcopy` is installed (the status line says which).

### Driving the TUI with tmux (automated)

To script a run (or let an agent run it), start each actor's app in its own detached tmux session, send keys with `send-keys`, and read the screen with `capture-pane`. Use a large window so the QR code fits (≥ 120×42):

```bash
B=$PWD/target/debug/frostpass; K=~/.config/solana/frostpass; for a in organizer holder buyer buyer2 scanner scanner2 scanner3; do tmux new-session -d -s $a -x 140 -y 48 "$B --keypair $K/$a.json"; sleep 1; done
```

Read a screen:

```bash
tmux capture-pane -p -t organizer
```

Send keys (examples):

```bash
tmux send-keys -t organizer 1          # Create tab
tmux send-keys -t organizer n          # new-event form
tmux send-keys -t organizer C-u        # clear the focused field
tmux send-keys -t organizer -l 'FrostPass QA'   # type literal text
tmux send-keys -t organizer Tab        # next field
tmux send-keys -t organizer C-s        # submit the form
tmux send-keys -t organizer Enter      # confirm (or y)
tmux send-keys -t organizer Escape     # cancel / back
```

After sending a key that triggers a network call, wait a few seconds before capturing, and re-capture until the status line settles (it shows `…` before the message while busy). Press `r` to refresh a window after another window changed the chain.

**Pasting a door pass** into the scanner: show the pass as text on the holder's pass screen (`t`), capture it, then type it into the scanner's form:

```bash
tmux send-keys -t scanner v
tmux send-keys -t scanner -l 'FROSTPASS:1:...'
tmux send-keys -t scanner Enter
```

The pass is upper-case base32 and may wrap across lines in the capture; join the lines (whitespace inside a pass is ignored by the decoder). A pass is valid for **60 seconds**, so redeem promptly; press `r` on the holder's pass screen for a fresh one.

Clean up at the end: `tmux kill-session -t <name>` for each actor.

## TUI reference

**Layout:** header (cluster, wallet, SOL, USDC, cluster clock) · tabs `1 Create │ 2 Ticket │ 3 Marketplace │ 4 Scanner │ 5 Log` · left: events, newest first (each shows its ID) · right: event details and tickets (the Door panel on the Scanner tab; the selected transaction on the Log tab) · key hints · status line.

| Keys | Where | Action |
|---|---|---|
| `1`–`5`, `Tab`, `Shift+Tab` | everywhere | switch tab |
| `/` (type, `Enter` keeps, `Esc` clears) | everywhere | search events by name (every word, any order) or ID; in the Log also by action or signature |
| `←` / `→` | everywhere | move between panes: events → tickets → door log (Create); events → door log (Scanner) |
| mouse | everywhere | click a tab to switch, click a pane to focus it and a row to select it, wheel to scroll; hold Shift to select text instead |
| `↑` `↓` (`k` `j`) | everywhere | select (or scroll, in the Log's details) |
| `r` · `?` · `q` | everywhere | refresh · help · quit |
| `n` | Create | create event (form) |
| `s` | Create | replace the scanner list (form) |
| `c` | Create | cancel event (type `cancel` to confirm) |
| `f` | Create | refund the selected ticket |
| `m` · `p` · `x` | Ticket | mint 1–3 tickets (form: how many; one transaction) · door pass · close |
| `b` · `l` · `d` | Marketplace | buy the selected listing at its price (no form) · list (form) · delist |
| `v` / paste, `Enter`, `Esc` | Scanner | check a pass · redeem if ADMIT · clear |
| `c` · `t` · `r` · `Esc` | pass screen | copy · QR/text · new pass · close |
| `r` · `c` | Log | check confirmations (fills in slot, fee, logs; marks finalized) · copy signature |
| `→`, `↑↓`, `PgUp`/`PgDn`, `Home`/`End`, `←` | Log | scroll the details and program logs · back to the list |
| `Tab`/`↓`, `Ctrl+S`, `Ctrl+U`, `Esc` | forms | next field · submit · clear field · cancel |

**Tab contents:** Create (organizers) shows only this wallet's events, all their tickets including burned ones, and the selected event's door log. Ticket shows all events and this wallet's tickets, including ones burned while it held them; only minting, door passes and closing happen here. Marketplace shows listings plus this wallet's live tickets; only buying, listing and delisting happen here (a key from the other tab says where to go). Every ticket table has a **Status** column: Held, Listed · price, or how it ended (Redeemed · when · by which scanner, Refunded, Closed); burned tickets come from the event's transaction history, loaded in the background (the title says "loading history…" meanwhile) and cached. Scanner shows only events where this wallet is a scanner, and the selected event's **Door log**: every ticket redeemed there by any of its scanners, with the time, the scanner and the holder, newest first (the same list for every scanner). Log shows every transaction this wallet sent from the app, also saved to `~/.local/share/frostpass/devnet/log-<wallet>.jsonl` (`$FROSTPASS_DATA` overrides the folder).

**Create-event form fields, in order:** Name · Metadata URI · Ticket price (USDC) · Supply · Resale cap (%) · Royalty (%) · Sales end · Event ends · Scanners (comma-separated) · Event ID (pre-filled random). Times accept `+90m`, `+2h`, `+3d` or `YYYY-MM-DD HH:MM` (UTC). Nothing fetches the metadata URI; any placeholder like `https://example.com/qa.json` works.

## Test plan

Use three events so the time-based tests can run in parallel. Create **B first**: its close-ticket test needs its event to have ended, which takes an hour. Write down each event's ID and each ticket's number as you go.

USDC budget: each mint at 1 USDC costs 1.25; a resale at the 10% cap costs 1.10. 10 USDC each for the holder and buyers covers the plan.

### 0. Smoke

- [ ] Each window starts, shows the right wallet with its SOL and USDC, and an event list (other people's devnet events may already exist).
- [ ] `?` opens help; any key closes it. Tabs switch with `1`–`5`.

### Event B: "QA Short" (sales and end timing)

Organizer: `n`, Name `QA Short`, URI `https://example.com/qa.json`, price `1`, supply `5`, cap `10`, royalty `5`, Sales end `+30m`, Event ends `+61m`, Scanners `<SCANNER>`.

- [ ] The confirm screen shows all values and "Creates your USDC account" (first event only). After confirming, the event appears in the Create tab, status **On sale**; the organizer now has a USDC account with 0.
- [ ] Buyer (Ticket tab): mint one ticket of QA Short (`m`, `1`). Buyer USDC −1.25; organizer +1; treasury +0.25 (`spl-token balance -u d --owner 9TRRTfqqfeMiACKETs6NUgdAamyxYLggNMbB2S1Jte97 4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`, compare before/after).
- [ ] **After sales end (~30 min):** status shows **Sales closed**. Minting → `Blocked … [SalesEnded]`; listing → `[SalesEnded]`. The buyer can still get a pass (`p`), and the scanner still sees **ADMIT** (don't redeem it if you want the close test below, or mint a second ticket earlier for this).
- [ ] **After event end (~61 min):** status **Ended**. Pass → `Blocked … [EventEnded]`. Another wallet can't close the buyer's ticket (it isn't in their tabs); the buyer closes it with `x` → the confirm warns it's burned; after confirming the ticket disappears and the buyer's SOL rises by the rents (~0.004 SOL).

### Event A: "QA Main" (primary sale, resale, door)

Organizer: `n`, Name `QA Main`, price `1`, supply `10`, cap `10`, royalty `5`, Sales end `+1d`, Event ends `+2d`, Scanners `<SCANNER>, <SCANNER2>`.

Form validation (before the real submit, each should show the error inside the form, nothing sent):
- [ ] Royalty `6` → `[ExceedsMaxAllowedRoyalty]`; cap `21` → `[ExceedsMaxAllowedMarkup]`.
- [ ] Sales end later than event end → `[InvalidSalesEnd]`; Event ends `+30m` → `[EventDurationTooShort]`.
- [ ] Event ends exactly an hour away as a clock time (e.g. now + 1h, `YYYY-MM-DD HH:MM`) → refused before sending with "… at least 1 hour after the transaction lands: <time> or later". Event ends `+1h` → passes, even after waiting a few minutes on the confirm screen (it counts from when you confirm).
- [ ] Same scanner twice → `[DuplicateScanner]`; empty name → `[InvalidNameLength]`.
- [ ] Reusing an existing event's ID (same organizer) → `[EventIdTaken]`.

Primary sale:
- [ ] Organizer tries to mint QA Main from the Ticket tab → `Blocked … [ConstraintDuplicateMutableAccount]`.
- [ ] Holder `m` → the form says `1–3`. Enter `4` → `[MintLimitReached]`. Enter `3` → confirm shows "Mint 3 tickets", `#1–#3`, total 3.75 USDC, "Your mints 3 of 3" → one transaction. Then `m` again → `[MintLimitReached]`. Holder USDC −3.75.
- [ ] Holder's tickets show owner "you" in the Ticket tab; the Create tab lists all tickets.

Resale (pick holder's ticket #1):
- [ ] Holder (Marketplace tab) `l` → the price field is pre-filled with the cap `1.10`. Enter `1.11` → `[PriceAboveCap]`. Enter `1.10` → confirm shows "you get 1.045, organizer 0.055 royalty" → listed.
- [ ] Holder `l` again on the same ticket → `[AlreadyListed]`. `d` delists; `l` relists at `1.10`.
- [ ] Buyer (Marketplace tab) sees the listing as **Listed · 1.10 USDC**. `b` goes straight to the confirm screen (no price to type): price 1.10, seller 1.045, royalty 0.055 to organizer, "Protocol fee: None on resales" → bought. (The listed price is also the transaction's limit: if the seller relists higher before you confirm, it's blocked with `[PriceExceedsMax]`.) Balances: buyer −1.10; holder +1.045; organizer +0.055; treasury unchanged.
- [ ] The ticket now shows owner "you" for the buyer and is gone from the holder's tickets.
- [ ] Buyer lists it at `1.10`; buyer 2 buys it (resale 2 of 2).
- [ ] Buyer 2 tries to list it → `[ResaleLimitReached]`.

Door pass and redeem (ticket #1, now buyer 2's):
- [ ] The holder and the buyer (previous owners) can't make a pass for it: it's no longer in their tickets.
- [ ] Buyer 2 `p` → pass screen with a QR code, "Valid for 60s". `t` shows the text form.
- [ ] Paste it into the **holder's** Scanner tab (the holder isn't a scanner): the tab shows no events, and the Door shows **REFUSE** with "Scanner: this wallet isn't a scanner of the event".
- [ ] Paste it into **scanner**: **ADMIT** with five ✓ (signature, scanner, event status, freshness, ownership). `Enter` → confirm → redeemed; "Redeemed this session (1)" lists it. The ticket disappears everywhere after refresh; buyer 2's SOL rises by the rents; the scanner's SOL drops only by the fee.
- [ ] Paste the **same pass again** → REFUSE ("Ticket: no ticket at … redeemed …").
- [ ] Ticket #1 stays listed where it belongs, as **Redeemed · <time> · by <scanner>**: in the organizer's All tickets (Create tab), and in buyer 2's Your tickets (Ticket tab; buyer 2 held it at the door). `p` or `x` on it → "… is redeemed: there's nothing left to do with it". It's in the **Door log** (time, ticket, which scanner redeemed it, the holder; newest first) for the organizer in the Create tab, without being a scanner, and for every scanner of QA Main (scanner and scanner 2) in the Scanner tab. It's still there after quitting and restarting the app.
- [ ] Wait > 60s with a fresh pass for another ticket, then paste → REFUSE, freshness ✗ ("expired").
- [ ] Tamper: change one character in the middle of a pass and paste → REFUSE (signature ✗) or "the pass is damaged".

Scanner add and replace:
- [ ] **Add:** Organizer `s` on QA Main → Scanners `<SCANNER>, <SCANNER2>, <SCANNER3>` → confirm shows "Added: <SCANNER3, shortened>" and no "Revoked" line → confirmed. After `r`, QA Main appears in scanner 3's Scanner tab.
- [ ] Holder makes a pass for ticket #2; scanner 3 → **ADMIT** (press `Esc`, don't redeem).
- [ ] **Replace:** Organizer `s` → Scanners `<SCANNER2>, <SCANNER3>` → confirm shows "Revoked: <SCANNER, shortened>" and no "Added" line.
- [ ] Fresh pass for ticket #2: scanner (revoked) → REFUSE, and QA Main disappears from scanner's Scanner tab after `r`. Scanner 3 → ADMIT → redeem.
- [ ] Holder makes a pass for ticket #3; scanner 2 (kept through both changes) → ADMIT → redeem.
- [ ] A non-organizer (e.g. the buyer) has no QA Main in their Create tab (can't reach `s`/`c`/`f`).

### Event C: "QA Cancel" (sold out, cancel, refund)

Organizer: `n`, Name `QA Cancel`, price `1`, supply `1`, cap `10`, royalty `5`, `+1d` / `+2d`, Scanners `<SCANNER>`.

- [ ] Holder mints the only ticket. Buyer tries to mint → `[EventSoldOut]`.
- [ ] Organizer `f` (refund) before cancelling → `[EventNotCancelled]`.
- [ ] Organizer `c` → confirm requires typing `cancel` (Enter does nothing until it matches) → cancelled; status **Cancelled** everywhere.
- [ ] Holder: mint → `[EventCancelled]`; pass → `[EventCancelled]`; list → `[EventCancelled]`.
- [ ] Organizer selects the holder's ticket (`→`, then select) and `f` → confirm shows refund 1.00 USDC to the holder, "the mint fee isn't refunded" → refunded. Holder USDC +1.00; organizer −1.00; the ticket is gone. (The organizer can afford it from earlier mint proceeds; if not, the app blocks with `[InsufficientUsdc]`.)
- [ ] `c` again → `[EventCancelled]` ("already cancelled").
- [ ] The refunded ticket shows as **Refunded** in the organizer's table and the holder's; QA Short's closed ticket shows as **Closed** for the organizer and the buyer.

### Ordering, search and the log

- [ ] Events list newest first: a just-created event is at the top of every tab. (On a machine's first run the title says "sorting newest first…" for up to a minute while it looks up each event once.)
- [ ] `/` then part of an event name (e.g. `qa main`, or `main qa`) narrows the list on every tab; its ID works too; no match shows "No events match"; `Esc` brings everything back.
- [ ] Log tab (`5`) after the holder's 3-ticket mint: one entry "Mint 3 tickets" with slot, block time, fee, compute units, SOL change (fee + rent for the new accounts), USDC change −3.75, three `FrostPass: mint_ticket` instructions and the program logs. It matches `solana confirm -u d -v <signature>`.
- [ ] `→` then `End` scrolls the details to the last program log line; `Home` goes back to the top; `←` returns to the list.
- [ ] A few seconds later, `r` in the Log tab → the entry turns **Finalized**. `/` with QA Main's name or ID filters the log to its transactions.
- [ ] A rejected transaction (if you hit one) is logged as **Failed** with the program error and logs.

### Robustness

- [ ] Resize a window small (`tmux resize-window -t holder -x 80 -y 24` for the automated sessions) and open a pass: it should say to enlarge the terminal for the QR and still show the text. Nothing should crash or garble permanently.
- [ ] Long values (a pasted pass, a scanner list) wrap inside forms and confirm screens, and the key hints stay visible.
- [ ] Press keys during a "…" busy status; the app shouldn't double-send.
- [ ] Note any RPC errors (429s) and whether the app recovers after `r`.

## Report

For each checklist item: **pass / fail / blocked**, plus for failures: the exact status line or screen capture, the transaction signature if any, and balances before/after. Then list:

1. Bugs (what you did, expected vs actual, how to reproduce).
2. UX issues (confusing text, layout problems at given sizes, missing feedback).
3. Any fix you made, with the diff and the results of `cargo test -p frost-pass-cli` and `cargo clippy -p frost-pass-cli --all-targets -- -D warnings`.
4. Final balances of all seven wallets.
