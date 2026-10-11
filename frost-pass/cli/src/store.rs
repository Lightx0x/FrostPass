//! What the app keeps on disk, per cluster: the slot each event was created in (so events can be
//! listed newest first without asking the cluster again), and each wallet's transaction log.
//!
//! ```text
//! ~/.local/share/frostpass/<cluster>/event-slots.txt     <event address> <slot>, one per line
//! ~/.local/share/frostpass/<cluster>/log-<wallet>.jsonl  one LogEntry per line, oldest first
//! ```
//!
//! Several windows (one per wallet) can run at once: files are replaced atomically, and the slot
//! cache merges with what's on disk before saving.

use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anchor_lang::prelude::Pubkey;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_signature::Signature;

use crate::{
    actions::{self, Plan},
    chain::{Confirmation, TxDetails, TxError},
    format,
};

/// `$FROSTPASS_DATA`, else `$XDG_DATA_HOME/frostpass`, else `~/.local/share/frostpass`; then the
/// cluster's own folder.
pub fn data_dir(cluster: &str) -> Option<PathBuf> {
    let base = match std::env::var_os("FROSTPASS_DATA") {
        Some(dir) => PathBuf::from(dir),
        None => match std::env::var_os("XDG_DATA_HOME") {
            Some(dir) => PathBuf::from(dir).join("frostpass"),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".local/share/frostpass"),
        },
    };
    Some(base.join(folder_name(cluster)))
}

/// A cluster name or URL as a folder name: `devnet`, or `http___127.0.0.1_8899`.
fn folder_name(cluster: &str) -> String {
    cluster
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Writes `contents` to `path` via a temporary file and a rename, so readers never see half a
/// file.
fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("couldn't create {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, contents).with_context(|| format!("couldn't write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("couldn't write {}", path.display()))
}

// ---------------------------------------------------------------------------
// Event creation slots
// ---------------------------------------------------------------------------

pub struct SlotCache {
    path: Option<PathBuf>,
    slots: HashMap<Pubkey, u64>,
}

impl SlotCache {
    /// A cache that only lives in memory.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            slots: HashMap::new(),
        }
    }

    pub fn open(dir: &Path) -> Self {
        let path = dir.join("event-slots.txt");
        Self {
            slots: read_slots(&path),
            path: Some(path),
        }
    }

    pub fn get(&self, address: &Pubkey) -> Option<u64> {
        self.slots.get(address).copied()
    }

    pub fn insert(&mut self, address: Pubkey, slot: u64) {
        self.slots.insert(address, slot);
    }

    /// Saves, keeping entries other windows added meanwhile.
    pub fn save(&mut self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        for (address, slot) in read_slots(path) {
            self.slots.entry(address).or_insert(slot);
        }
        let mut lines: Vec<String> = self
            .slots
            .iter()
            .map(|(address, slot)| format!("{address} {slot}\n"))
            .collect();
        lines.sort();
        write_atomic(path, &lines.concat())
    }
}

fn read_slots(path: &Path) -> HashMap<Pubkey, u64> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (address, slot) = line.split_once(' ')?;
            Some((address.parse().ok()?, slot.trim().parse().ok()?))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Transaction log
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStatus {
    /// Sent and confirmed; the cluster's details may still be missing (press r in the Log tab).
    Confirmed,
    /// Rooted: it can't be rolled back.
    Finalized,
    /// The program (or the cluster) rejected it. Nothing changed on chain.
    Failed,
}

impl LogStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "Confirmed",
            Self::Finalized => "Finalized",
            Self::Failed => "Failed",
        }
    }
}

/// One transaction sent from this wallet: what it was for, and what the cluster recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// When it was sent, by this computer's clock (unix seconds).
    pub sent_at: i64,
    pub cluster: String,
    pub wallet: String,
    /// The action, as on the confirmation screen ("Mint 3 tickets").
    pub action: String,
    pub event_id: Option<u32>,
    /// Every instruction, in order: `FrostPass: mint_ticket`, `Ed25519: verify`, …
    pub instructions: Vec<String>,
    /// The confirmation screen's lines (amounts, recipients, ticket numbers).
    pub details: Vec<(String, String)>,
    pub status: LogStatus,
    pub signature: Option<String>,
    pub slot: Option<u64>,
    /// When the block was produced, by the cluster (unix seconds).
    pub block_time: Option<i64>,
    /// Lamports.
    pub fee: Option<u64>,
    pub compute_units: Option<u64>,
    /// This wallet's SOL change in lamports, fee included.
    pub sol_change: Option<i64>,
    /// This wallet's USDC change in base units.
    pub usdc_change: Option<i64>,
    /// Why it failed: the program's error when there is one.
    pub error: Option<String>,
    pub logs: Vec<String>,
}

impl LogEntry {
    /// An entry for `plan`, before it's sent.
    pub fn new(plan: &Plan, wallet: &Pubkey, cluster: &str, now: i64) -> Self {
        Self {
            sent_at: now,
            cluster: cluster.to_string(),
            wallet: wallet.to_string(),
            action: plan.title.clone(),
            event_id: plan.event_id,
            instructions: actions::instruction_names(&plan.instructions),
            details: plan.lines.clone(),
            status: LogStatus::Confirmed,
            signature: None,
            slot: None,
            block_time: None,
            fee: None,
            compute_units: None,
            sol_change: None,
            usdc_change: None,
            error: None,
            logs: Vec::new(),
        }
    }

    pub fn landed(&mut self, signature: &Signature) {
        self.status = LogStatus::Confirmed;
        self.signature = Some(signature.to_string());
    }

    pub fn failed(&mut self, error: &anyhow::Error, description: String) {
        self.status = LogStatus::Failed;
        self.error = Some(description);
        if let Some(tx) = error.downcast_ref::<TxError>() {
            self.logs = tx.logs.clone();
        }
    }

    pub fn add_details(&mut self, details: TxDetails) {
        self.slot = Some(details.slot);
        self.block_time = details.block_time;
        self.fee = Some(details.fee);
        self.compute_units = details.compute_units;
        self.sol_change = Some(details.sol_change);
        self.usdc_change = Some(details.usdc_change);
        self.logs = details.logs;
    }

    pub fn confirm(&mut self, confirmation: Confirmation) {
        if self.status != LogStatus::Failed && confirmation == Confirmation::Finalized {
            self.status = LogStatus::Finalized;
        }
    }

    pub fn signature(&self) -> Option<Signature> {
        self.signature.as_deref()?.parse().ok()
    }

    /// Whether a later refresh can still add something: the cluster's details, or finality.
    pub fn unsettled(&self) -> bool {
        self.signature.is_some() && (self.slot.is_none() || self.status == LogStatus::Confirmed)
    }

    /// Search: words match the event's name and the action (every word, any order); a number
    /// is an event ID; a long code is the start of the signature. (Digits would otherwise match
    /// every amount and signature.)
    pub fn matches(&self, query: &str) -> bool {
        if query.chars().all(|c| c.is_ascii_digit()) {
            return self
                .event_id
                .is_some_and(|id| id.to_string().contains(query));
        }
        let event = self
            .details
            .iter()
            .find(|(label, _)| label == "Event" || label == "Name")
            .map_or("", |(_, value)| value.as_str());
        format::matches_words(query, &format!("{event} {}", self.action))
            || self
                .signature
                .as_deref()
                .is_some_and(|s| query.len() >= 4 && s.starts_with(query))
    }
}

/// This wallet's transactions, oldest first in the file and in `entries`.
pub struct TxLog {
    path: Option<PathBuf>,
    pub entries: Vec<LogEntry>,
}

impl TxLog {
    /// A log that only lives in memory (tests, or when there's no data folder).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            entries: Vec::new(),
        }
    }

    pub fn open(dir: &Path, wallet: &Pubkey) -> Self {
        let path = dir.join(format!("log-{wallet}.jsonl"));
        let entries = fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        Self {
            path: Some(path),
            entries,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn push(&mut self, entry: LogEntry) -> Result<()> {
        let line = serde_json::to_string(&entry)? + "\n";
        self.entries.push(entry);
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("couldn't create {}", parent.display()))?;
        }
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(line.as_bytes()))
            .with_context(|| format!("couldn't write {}", path.display()))
    }

    /// Rewrites the file after entries were updated in place.
    pub fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut contents = String::new();
        for entry in &self.entries {
            contents += &serde_json::to_string(entry)?;
            contents.push('\n');
        }
        write_atomic(path, &contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("frostpass-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn slots_and_log_survive_a_restart() {
        let dir = temp_dir("store");
        let event = Pubkey::new_unique();
        let mut slots = SlotCache::open(&dir);
        slots.insert(event, 42);
        slots.save().unwrap();
        assert_eq!(SlotCache::open(&dir).get(&event), Some(42));

        let wallet = Pubkey::new_unique();
        let mut log = TxLog::open(&dir, &wallet);
        let mut entry = LogEntry::new(&Plan::new("Mint ticket"), &wallet, "devnet", 1);
        entry.event_id = Some(330702);
        entry.landed(&Signature::default());
        log.push(entry.clone()).unwrap();
        entry.confirm(Confirmation::Finalized);
        log.entries[0] = entry.clone();
        log.save().unwrap();

        let reopened = TxLog::open(&dir, &wallet);
        assert_eq!(reopened.entries, vec![entry.clone()]);
        assert_eq!(reopened.entries[0].status, LogStatus::Finalized);
        assert!(reopened.entries[0].matches("3307"));
        assert!(reopened.entries[0].matches("mint"));
        assert!(!reopened.entries[0].matches("refund"));
        entry.details = vec![("Event".into(), "Light Tech Meetup · ID 330702".into())];
        assert!(entry.matches("tech light"));
        assert!(entry.matches("meetup mint"));
        assert!(!entry.matches("tech concert"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cluster_urls_become_folder_names() {
        assert_eq!(folder_name("devnet"), "devnet");
        assert_eq!(
            folder_name("http://127.0.0.1:8899"),
            "http___127.0.0.1_8899"
        );
    }
}
