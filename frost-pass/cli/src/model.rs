//! FrostPass accounts as the app sees them, and the queries that load them.
//!
//! Tickets are validated the way the program's `load_ticket_asset` does it (a Core `AssetV1`
//! whose update authority is the event's collection) plus the `TicketState` event check, so the
//! app never shows or acts on an asset the program would reject.

use std::collections::HashMap;

use anchor_lang::{
    prelude::Pubkey, solana_program::program_pack::Pack, AccountDeserialize, Discriminator,
};
use anchor_spl::token::spl_token::state::Account as TokenAccount;
use anyhow::{bail, ensure, Context, Result};
use frost_pass::{
    constants::USDC_MINT,
    state::{EventConfig, MinterRecord, TicketState},
    ID as FROST_PASS_ID,
};
use frost_pass_client::{minter_record_pda, ticket_state_pda, usdc_ata, Event};
use mpl_core::{
    accounts::{BaseAssetV1, BaseCollectionV1},
    types::{Key, UpdateAuthority},
    ID as MPL_CORE_ID,
};
use solana_account::Account;

use crate::chain::{Chain, Filter};

/// Where an event is in its lifecycle, by the cluster clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventStatus {
    /// Before `sales_end`: mint, list and buy are open; tickets can be redeemed.
    OnSale,
    /// Between `sales_end` and `event_end`: no sales, but tickets can still be redeemed.
    SalesClosed,
    /// After `event_end`: owners can close their tickets.
    Ended,
    /// Cancelled by the organizer: only refunds, delisting and closing remain.
    Cancelled,
}

impl EventStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::OnSale => "On sale",
            Self::SalesClosed => "Sales closed",
            Self::Ended => "Ended",
            Self::Cancelled => "Cancelled",
        }
    }
}

#[derive(Clone)]
pub struct EventInfo {
    pub address: Pubkey,
    pub config: EventConfig,
    /// The collection's name and URI (the event's metadata lives on its Core collection).
    pub name: String,
    pub uri: String,
}

impl EventInfo {
    pub fn event(&self) -> Event {
        Event {
            config: self.address,
            collection: self.config.collection,
            organizer: self.config.organizer,
        }
    }

    pub fn status(&self, now: i64) -> EventStatus {
        if self.config.cancelled {
            EventStatus::Cancelled
        } else if now >= self.config.event_end {
            EventStatus::Ended
        } else if now >= self.config.sales_end {
            EventStatus::SalesClosed
        } else {
            EventStatus::OnSale
        }
    }

    pub fn tickets_left(&self) -> u32 {
        self.config
            .ticket_supply
            .saturating_sub(self.config.tickets_minted)
    }
}

#[derive(Clone)]
pub struct TicketInfo {
    pub asset: Pubkey,
    pub owner: Pubkey,
    pub name: String,
    pub state: TicketState,
}

impl TicketInfo {
    /// The listing, if any: `(seller, price)`.
    pub fn listing(&self) -> Option<(Pubkey, u64)> {
        self.state
            .seller
            .map(|seller| (seller, self.state.list_price))
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

fn anchor_account<T: AccountDeserialize>(account: &Account, what: &str) -> Result<T> {
    ensure!(
        account.owner == FROST_PASS_ID,
        "{what} isn't owned by the FrostPass program"
    );
    T::try_deserialize(&mut account.data.as_slice()).with_context(|| format!("invalid {what}"))
}

/// A Core asset that is a ticket of the event whose collection is `collection`.
pub fn decode_ticket_asset(account: &Account, collection: &Pubkey) -> Result<BaseAssetV1> {
    ensure!(account.owner == MPL_CORE_ID, "not a Metaplex Core account");
    let asset = BaseAssetV1::from_bytes(&account.data).context("not a Metaplex Core asset")?;
    // `from_bytes` doesn't check the account type, so reject collections and other Core accounts
    ensure!(asset.key == Key::AssetV1, "not a Metaplex Core asset");
    ensure!(
        asset.update_authority == UpdateAuthority::Collection(*collection),
        "the asset isn't a ticket of this event"
    );
    Ok(asset)
}

/// The USDC balance of a token account, checking it's really a USDC account of `owner`.
pub fn decode_usdc(account: &Account, owner: &Pubkey) -> Result<u64> {
    let token = TokenAccount::unpack(&account.data).context("not a token account")?;
    ensure!(
        token.mint == USDC_MINT && token.owner == *owner,
        "not the USDC account of {owner}"
    );
    Ok(token.amount)
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Byte offset of `EventConfig.organizer` (after the discriminator and the `u32` event id).
fn organizer_offset() -> usize {
    EventConfig::DISCRIMINATOR.len() + 4
}

pub fn event(chain: &impl Chain, address: &Pubkey) -> Result<EventInfo> {
    let account = chain
        .account(address)?
        .with_context(|| format!("no event at {address}"))?;
    let config: EventConfig = anchor_account(&account, "event")?;
    let mut events = with_names(chain, vec![(*address, config)])?;
    Ok(events.remove(0))
}

/// Every event, or only `organizer`'s: cancelled events last, the rest latest-ending first (so
/// upcoming events come before past ones).
pub fn events(chain: &impl Chain, organizer: Option<&Pubkey>) -> Result<Vec<EventInfo>> {
    let mut filters = vec![Filter::new(0, EventConfig::DISCRIMINATOR)];
    if let Some(organizer) = organizer {
        filters.push(Filter::new(organizer_offset(), organizer.to_bytes()));
    }
    let configs = chain
        .program_accounts(&FROST_PASS_ID, &filters)?
        .into_iter()
        .filter_map(|(address, account)| {
            anchor_account::<EventConfig>(&account, "event")
                .ok()
                .map(|config| (address, config))
        })
        .collect();
    let mut events = with_names(chain, configs)?;
    events.sort_by_key(|event| {
        (
            event.config.cancelled,
            std::cmp::Reverse(event.config.event_end),
        )
    });
    Ok(events)
}

/// Newest first by the slot each event was created in. The sort is stable and events with no
/// known slot go last, so they keep the order [`events`] gave them.
pub fn sort_newest_first(events: &mut [EventInfo], slot: impl Fn(&Pubkey) -> Option<u64>) {
    events.sort_by_key(|event| {
        let slot = slot(&event.address);
        (slot.is_none(), std::cmp::Reverse(slot))
    });
}

/// Adds each event's name and URI from its collection.
fn with_names(chain: &impl Chain, configs: Vec<(Pubkey, EventConfig)>) -> Result<Vec<EventInfo>> {
    let collections: Vec<Pubkey> = configs.iter().map(|(_, c)| c.collection).collect();
    let accounts = chain.accounts(&collections)?;
    Ok(configs
        .into_iter()
        .zip(accounts)
        .map(|((address, config), account)| {
            let collection = account
                .filter(|account| account.owner == MPL_CORE_ID)
                .and_then(|account| BaseCollectionV1::from_bytes(&account.data).ok());
            let (name, uri) = collection
                .map(|c| (c.name, c.uri))
                .unwrap_or_else(|| (format!("Event #{}", config.event_id), String::new()));
            EventInfo {
                address,
                config,
                name,
                uri,
            }
        })
        .collect())
}

/// Every live ticket of `event`: the Core assets in its collection with their `TicketState`.
/// Burned tickets (redeemed, refunded, closed) are gone.
pub fn event_tickets(chain: &impl Chain, event: &EventInfo) -> Result<Vec<TicketInfo>> {
    // AssetV1 layout: key (1 byte), owner (32), update authority tag (1) + address (32)
    let mut collection_authority = vec![2u8]; // UpdateAuthority::Collection
    collection_authority.extend_from_slice(event.config.collection.as_ref());
    let assets = chain.program_accounts(
        &MPL_CORE_ID,
        &[
            Filter::new(0, [Key::AssetV1 as u8]),
            Filter::new(33, collection_authority),
        ],
    )?;

    let state_addresses: Vec<Pubkey> = assets
        .iter()
        .map(|(asset, _)| ticket_state_pda(asset))
        .collect();
    let states = chain.accounts(&state_addresses)?;

    let mut tickets: Vec<TicketInfo> = assets
        .into_iter()
        .zip(states)
        .filter_map(|((address, account), state)| {
            let asset = decode_ticket_asset(&account, &event.config.collection).ok()?;
            let state: TicketState = anchor_account(&state?, "ticket state").ok()?;
            (state.event == event.address).then_some(TicketInfo {
                asset: address,
                owner: asset.owner,
                name: asset.name,
                state,
            })
        })
        .collect();
    tickets.sort_by_key(|ticket| ticket_number(&ticket.name));
    Ok(tickets)
}

/// The number at the end of a ticket's name (`"Coldplay #12"` → 12), for sorting.
fn ticket_number(name: &str) -> u32 {
    name.rsplit('#')
        .next()
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(u32::MAX)
}

/// One ticket and its event, validated the way the program validates it.
pub fn ticket(chain: &impl Chain, asset: &Pubkey) -> Result<(EventInfo, TicketInfo)> {
    let [asset_account, state_account] = chain
        .accounts(&[*asset, ticket_state_pda(asset)])?
        .try_into()
        .map_err(|_| anyhow::anyhow!("unexpected RPC response"))?;
    let Some(asset_account) = asset_account else {
        bail!("no ticket at {asset} (it may have been redeemed, refunded or closed)");
    };
    let state: TicketState = anchor_account(
        &state_account.context("this asset isn't a FrostPass ticket")?,
        "ticket state",
    )?;
    let event = event(chain, &state.event)?;
    let core_asset = decode_ticket_asset(&asset_account, &event.config.collection)?;
    Ok((
        event,
        TicketInfo {
            asset: *asset,
            owner: core_asset.owner,
            name: core_asset.name,
            state,
        },
    ))
}

/// The wallet's USDC balance, or `None` if it has no USDC account.
pub fn usdc_balance(chain: &impl Chain, owner: &Pubkey) -> Result<Option<u64>> {
    chain
        .account(&usdc_ata(owner))?
        .map(|account| decode_usdc(&account, owner))
        .transpose()
}

pub fn sol_balance(chain: &impl Chain, address: &Pubkey) -> Result<u64> {
    Ok(chain.account(address)?.map_or(0, |a| a.lamports))
}

/// Primary mints `wallet` has made for `event`.
pub fn mints_by(chain: &impl Chain, event: &Pubkey, wallet: &Pubkey) -> Result<u8> {
    match chain.account(&minter_record_pda(event, wallet))? {
        Some(account) => Ok(anchor_account::<MinterRecord>(&account, "mint record")?.count),
        None => Ok(0),
    }
}

/// Which of `wallets` have a USDC account.
pub fn usdc_accounts_exist(
    chain: &impl Chain,
    wallets: &[Pubkey],
) -> Result<HashMap<Pubkey, bool>> {
    let atas: Vec<Pubkey> = wallets.iter().map(usdc_ata).collect();
    Ok(wallets
        .iter()
        .copied()
        .zip(chain.accounts(&atas)?.into_iter().map(|a| a.is_some()))
        .collect())
}
