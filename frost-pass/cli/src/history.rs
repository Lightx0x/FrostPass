//! Ticket histories, rebuilt from transactions.
//!
//! A burned ticket (redeemed, refunded or closed) leaves no account behind, so its story only
//! survives in the transactions that touched it. This module reads those transactions for an
//! address (an event, or a scanner's wallet), decodes the FrostPass instructions in them into
//! [`TicketEvent`]s, and folds them into one [`TicketRecord`] per ticket: its number, who held
//! it, and how it ended.
//!
//! Landed transactions never change, so a [`History`] is synced incrementally (new signatures
//! only, one transaction per step) and cached on disk in `<data dir>/history/<address>.json`.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anchor_lang::{prelude::Pubkey, Discriminator};
use anyhow::Result;
use frost_pass::{instruction as ix, ID as FROST_PASS_ID};
use mpl_core::ID as MPL_CORE_ID;
use serde::{Deserialize, Serialize};
use solana_signature::Signature;

use crate::{
    chain::{Chain, RawInstruction, TxRecord},
    store,
};

/// What happened to a ticket in one instruction. Addresses are base58 strings (for the cache).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// `name` is the Core asset's name (`"Event #3"`), from the mint's `CreateV1`.
    Minted {
        minter: String,
        name: Option<String>,
    },
    Listed {
        seller: String,
        price: u64,
    },
    /// `max_price` is the buyer's limit; the price paid is the listing's.
    Bought {
        buyer: String,
        seller: String,
        max_price: u64,
    },
    Redeemed {
        scanner: String,
        holder: String,
    },
    Refunded {
        holder: String,
    },
    Closed {
        holder: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketEvent {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    /// The event's config account.
    pub event: String,
    pub asset: String,
    pub action: Action,
}

impl TicketEvent {
    pub fn asset(&self) -> Option<Pubkey> {
        self.asset.parse().ok()
    }

    pub fn event(&self) -> Option<Pubkey> {
        self.event.parse().ok()
    }
}

fn u64_arg(data: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(8..16)?.try_into().ok()?))
}

/// The name in an mpl-core `CreateV1`: discriminator 0, data state (1 byte), then a borsh string.
fn created_name(instruction: &RawInstruction) -> Option<String> {
    let data = &instruction.data;
    if instruction.program != MPL_CORE_ID || data.first() != Some(&0) {
        return None;
    }
    let len = u32::from_le_bytes(data.get(2..6)?.try_into().ok()?) as usize;
    String::from_utf8(data.get(6..6 + len)?.to_vec()).ok()
}

/// The ticket events in one transaction, in instruction order. Instructions that don't involve
/// a specific ticket (create, cancel, scanner changes) and delists (which don't name the asset)
/// are skipped.
pub fn decode(record: &TxRecord) -> Vec<TicketEvent> {
    record
        .instructions
        .iter()
        .filter(|instruction| instruction.program == FROST_PASS_ID)
        .filter_map(|instruction| {
            let data = &instruction.data;
            let account = |i: usize| instruction.accounts.get(i).map(Pubkey::to_string);
            let is = |discriminator: &[u8]| data.starts_with(discriminator);
            // (event, asset, action), by each instruction's account order
            let (event, asset, action) = if is(ix::MintTicket::DISCRIMINATOR) {
                let name = instruction.inner.iter().find_map(created_name);
                (
                    account(1)?,
                    account(3)?,
                    Action::Minted {
                        minter: account(0)?,
                        name,
                    },
                )
            } else if is(ix::ListTicket::DISCRIMINATOR) {
                (
                    account(1)?,
                    account(2)?,
                    Action::Listed {
                        seller: account(0)?,
                        price: u64_arg(data)?,
                    },
                )
            } else if is(ix::BuyTicket::DISCRIMINATOR) {
                (
                    account(2)?,
                    account(4)?,
                    Action::Bought {
                        buyer: account(0)?,
                        seller: account(1)?,
                        max_price: u64_arg(data)?,
                    },
                )
            } else if is(ix::RedeemTicket::DISCRIMINATOR) {
                (
                    account(2)?,
                    account(4)?,
                    Action::Redeemed {
                        scanner: account(0)?,
                        holder: account(1)?,
                    },
                )
            } else if is(ix::RefundTicket::DISCRIMINATOR) {
                (
                    account(2)?,
                    account(4)?,
                    Action::Refunded {
                        holder: account(1)?,
                    },
                )
            } else if is(ix::CloseTicket::DISCRIMINATOR) {
                (
                    account(1)?,
                    account(3)?,
                    Action::Closed {
                        holder: account(0)?,
                    },
                )
            } else {
                return None;
            };
            Some(TicketEvent {
                signature: record.signature.to_string(),
                slot: record.slot,
                block_time: record.block_time,
                event,
                asset,
                action,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// One record per ticket
// ---------------------------------------------------------------------------

/// How a ticket ended, if it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fate {
    /// Still exists (as far as this history goes).
    Live,
    Redeemed {
        scanner: Pubkey,
        at: Option<i64>,
        signature: String,
    },
    Refunded {
        at: Option<i64>,
    },
    Closed {
        at: Option<i64>,
    },
}

impl Fate {
    pub fn is_live(&self) -> bool {
        *self == Fate::Live
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketRecord {
    pub asset: Pubkey,
    pub event: Pubkey,
    /// `"Event #3"`, when the mint was seen.
    pub name: Option<String>,
    pub minter: Option<Pubkey>,
    /// Who held it last: the current owner if it's live, else the owner when it was burned.
    pub holder: Option<Pubkey>,
    /// Completed resales.
    pub resales: u8,
    /// The last listing price, for the price a buyer paid.
    pub last_price: Option<u64>,
    pub fate: Fate,
}

impl TicketRecord {
    /// The number at the end of the name (`"Event #3"` → 3).
    pub fn number(&self) -> Option<u32> {
        self.name
            .as_deref()?
            .rsplit('#')
            .next()?
            .trim()
            .parse()
            .ok()
    }
}

/// Folds events (oldest first) into one record per ticket, by asset.
pub fn tickets<'a>(
    events: impl IntoIterator<Item = &'a TicketEvent>,
) -> HashMap<Pubkey, TicketRecord> {
    let mut tickets: HashMap<Pubkey, TicketRecord> = HashMap::new();
    for event in events {
        let (Some(asset), Some(event_address)) = (event.asset(), event.event()) else {
            continue;
        };
        let record = tickets.entry(asset).or_insert_with(|| TicketRecord {
            asset,
            event: event_address,
            name: None,
            minter: None,
            holder: None,
            resales: 0,
            last_price: None,
            fate: Fate::Live,
        });
        let key = |s: &String| s.parse::<Pubkey>().ok();
        match &event.action {
            Action::Minted { minter, name } => {
                record.minter = key(minter);
                record.holder = key(minter);
                if name.is_some() {
                    record.name.clone_from(name);
                }
            }
            Action::Listed { price, .. } => record.last_price = Some(*price),
            Action::Bought { buyer, .. } => {
                record.holder = key(buyer);
                record.resales = record.resales.saturating_add(1);
            }
            Action::Redeemed { scanner, holder } => {
                record.holder = key(holder);
                record.fate = Fate::Redeemed {
                    scanner: key(scanner).unwrap_or_default(),
                    at: event.block_time,
                    signature: event.signature.clone(),
                };
            }
            Action::Refunded { holder } => {
                record.holder = key(holder);
                record.fate = Fate::Refunded {
                    at: event.block_time,
                };
            }
            Action::Closed { holder } => {
                record.holder = key(holder);
                record.fate = Fate::Closed {
                    at: event.block_time,
                };
            }
        }
    }
    tickets
}

// ---------------------------------------------------------------------------
// Syncing and caching
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryTx {
    pub signature: String,
    pub slot: u64,
    pub events: Vec<TicketEvent>,
}

/// The ticket events of every transaction that touched one address, oldest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    /// The newest signature listed so far: the next listing starts after it.
    newest: Option<String>,
    /// Listed but not fetched yet, oldest first.
    pending: Vec<String>,
    pub txs: Vec<HistoryTx>,
    /// Whether this run has listed new signatures yet (a refresh clears it).
    #[serde(skip)]
    listed: bool,
    /// The cluster didn't serve a transaction yet; try again after a refresh.
    #[serde(skip)]
    stalled: bool,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl History {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn open(dir: &Path, address: &Pubkey) -> Self {
        let path = dir.join("history").join(format!("{address}.json"));
        let mut history: Self = fs::read_to_string(&path)
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        history.path = Some(path);
        history
    }

    pub fn events(&self) -> impl Iterator<Item = &TicketEvent> {
        self.txs.iter().flat_map(|tx| &tx.events)
    }

    /// Whether [`Self::step`] has something to do.
    pub fn needs_sync(&self) -> bool {
        !self.stalled && (!self.listed || !self.pending.is_empty())
    }

    /// Fetching (the newest transactions may still be missing).
    pub fn syncing(&self) -> bool {
        !self.listed || !self.pending.is_empty()
    }

    /// Stops syncing until the next [`Self::rescan`] (after an RPC error).
    pub fn stall(&mut self) {
        self.stalled = true;
    }

    /// Look for new transactions on the next step.
    pub fn rescan(&mut self) {
        self.listed = false;
        self.stalled = false;
    }

    /// One unit of work: list new signatures, or fetch one transaction. A single RPC call
    /// (a page of signatures, or one transaction), so it can run between key presses.
    pub fn step(&mut self, chain: &impl Chain, address: &Pubkey) -> Result<()> {
        if !self.listed {
            let until = self
                .newest
                .as_deref()
                .and_then(|s| s.parse::<Signature>().ok());
            let listed = chain.signatures(address, until.as_ref())?;
            if let Some(newest) = listed.first() {
                self.newest = Some(newest.signature.to_string());
            }
            // Oldest first; failed transactions changed nothing
            for entry in listed.iter().rev().filter(|entry| !entry.failed) {
                let signature = entry.signature.to_string();
                if !self.pending.contains(&signature) {
                    self.pending.push(signature);
                }
            }
            self.listed = true;
            return self.save();
        }
        let Some(signature) = self.pending.first().cloned() else {
            return Ok(());
        };
        match chain.record(&signature.parse()?)? {
            Some(record) => {
                self.pending.remove(0);
                self.txs.push(HistoryTx {
                    signature,
                    slot: record.slot,
                    events: decode(&record),
                });
                self.save()
            }
            None => {
                self.stalled = true;
                Ok(())
            }
        }
    }

    fn save(&self) -> Result<()> {
        match &self.path {
            Some(path) => store::write_atomic(path, &serde_json::to_string(self)?),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint_name(name: &str) -> RawInstruction {
        let mut data = vec![0, 0];
        data.extend((name.len() as u32).to_le_bytes());
        data.extend(name.as_bytes());
        RawInstruction {
            program: MPL_CORE_ID,
            accounts: Vec::new(),
            data,
            inner: Vec::new(),
        }
    }

    #[test]
    fn a_tickets_story_folds_into_one_record() {
        let [minter, buyer, scanner, event, collection, asset] =
            std::array::from_fn(|_| Pubkey::new_unique());
        let frost = |data: Vec<u8>, accounts: Vec<Pubkey>, inner| RawInstruction {
            program: FROST_PASS_ID,
            accounts,
            data,
            inner,
        };
        let with_u64 = |discriminator: &[u8], value: u64| {
            let mut data = discriminator.to_vec();
            data.extend(value.to_le_bytes());
            data
        };
        let tx = |slot, instructions| TxRecord {
            signature: Signature::from([slot as u8; 64]),
            slot,
            block_time: Some(slot as i64 * 10),
            instructions,
        };
        let txs = [
            tx(
                1,
                vec![frost(
                    ix::MintTicket::DISCRIMINATOR.to_vec(),
                    vec![minter, event, collection, asset],
                    vec![mint_name("Gig #7")],
                )],
            ),
            tx(
                2,
                vec![frost(
                    with_u64(ix::ListTicket::DISCRIMINATOR, 1_100_000),
                    vec![minter, event, asset],
                    vec![],
                )],
            ),
            tx(
                3,
                vec![frost(
                    with_u64(ix::BuyTicket::DISCRIMINATOR, 1_100_000),
                    vec![buyer, minter, event, collection, asset],
                    vec![],
                )],
            ),
            tx(
                4,
                vec![frost(
                    with_u64(ix::RedeemTicket::DISCRIMINATOR, 0),
                    vec![scanner, buyer, event, collection, asset],
                    vec![],
                )],
            ),
        ];
        let events: Vec<TicketEvent> = txs.iter().flat_map(decode).collect();
        assert_eq!(events.len(), 4);
        let record = &tickets(&events)[&asset];
        assert_eq!(record.number(), Some(7));
        assert_eq!(record.minter, Some(minter));
        assert_eq!(record.holder, Some(buyer));
        assert_eq!(record.resales, 1);
        assert_eq!(record.last_price, Some(1_100_000));
        assert!(matches!(
            record.fate,
            Fate::Redeemed { scanner: s, at: Some(40), .. } if s == scanner
        ));
    }
}
