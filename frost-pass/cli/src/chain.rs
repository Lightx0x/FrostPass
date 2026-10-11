//! Access to the cluster: reading accounts, finding program accounts and sending transactions.
//!
//! The app talks to a cluster over RPC ([`RpcChain`]); the tests implement [`Chain`] on LiteSVM,
//! so every action runs the same code in both.

use anchor_lang::{prelude::Pubkey, pubkey, solana_program::instruction::Instruction};
use anyhow::{anyhow, Context, Result};
use solana_account::Account;
use solana_account_decoder_client_types::UiAccountEncoding;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_api::{
    client_error::{Error as ClientError, ErrorKind as ClientErrorKind},
    config::{RpcAccountInfoConfig, RpcProgramAccountsConfig},
    filter::{Memcmp, RpcFilterType},
    request::{RpcError, RpcResponseErrorData},
};
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::Transaction;

/// The clock sysvar.
const CLOCK_SYSVAR: Pubkey = pubkey!("SysvarC1ock11111111111111111111111111111111");

/// `getMultipleAccounts` accepts at most 100 addresses per call.
const MAX_MULTIPLE_ACCOUNTS: usize = 100;

/// A byte filter for [`Chain::program_accounts`]: the account data at `offset` starts with `bytes`.
#[derive(Clone, Debug)]
pub struct Filter {
    pub offset: usize,
    pub bytes: Vec<u8>,
}

impl Filter {
    pub fn new(offset: usize, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            offset,
            bytes: bytes.into(),
        }
    }

    pub fn matches(&self, data: &[u8]) -> bool {
        data.get(self.offset..self.offset + self.bytes.len()) == Some(&self.bytes[..])
    }
}

/// A transaction the cluster rejected, with the program logs when there are any.
#[derive(Debug)]
pub struct TxError {
    pub message: String,
    pub logs: Vec<String>,
}

impl TxError {
    /// The Anchor error the program returned, e.g. `"SalesEnded (6005): Ticket sales have ended"`.
    pub fn program_error(&self) -> Option<String> {
        self.logs.iter().find_map(|line| {
            let rest = line.split("Error Code: ").nth(1)?;
            let (name, rest) = rest.split_once(". Error Number: ")?;
            let (number, message) = rest.split_once(". Error Message: ")?;
            Some(format!(
                "{name} ({number}): {}",
                message.trim_end_matches('.')
            ))
        })
    }
}

impl std::fmt::Display for TxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.program_error() {
            Some(error) => write!(f, "rejected by the program: {error}"),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for TxError {}

pub trait Chain {
    fn account(&self, address: &Pubkey) -> Result<Option<Account>>;

    fn accounts(&self, addresses: &[Pubkey]) -> Result<Vec<Option<Account>>> {
        addresses
            .iter()
            .map(|address| self.account(address))
            .collect()
    }

    /// Accounts owned by `program` whose data matches every filter.
    fn program_accounts(
        &self,
        program: &Pubkey,
        filters: &[Filter],
    ) -> Result<Vec<(Pubkey, Account)>>;

    /// Signs with `signers` (the first one pays) and sends; returns once confirmed.
    fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Signature, TxError>;

    /// The cluster's clock. All time checks use it, never the local clock.
    fn now(&self) -> Result<i64> {
        let clock = self
            .account(&CLOCK_SYSVAR)?
            .context("the cluster has no clock sysvar")?;
        // Clock layout: slot, epoch_start_timestamp, epoch, leader_schedule_epoch, unix_timestamp
        let bytes = clock
            .data
            .get(32..40)
            .context("unexpected clock sysvar layout")?;
        Ok(i64::from_le_bytes(bytes.try_into()?))
    }
}

pub struct RpcChain {
    client: RpcClient,
}

impl RpcChain {
    pub fn new(url: &str) -> Self {
        Self {
            client: RpcClient::new_with_commitment(url.to_string(), CommitmentConfig::confirmed()),
        }
    }

    pub fn url(&self) -> String {
        self.client.url()
    }
}

impl Chain for RpcChain {
    fn account(&self, address: &Pubkey) -> Result<Option<Account>> {
        Ok(self
            .client
            .get_account_with_commitment(address, self.client.commitment())?
            .value)
    }

    fn accounts(&self, addresses: &[Pubkey]) -> Result<Vec<Option<Account>>> {
        let mut accounts = Vec::with_capacity(addresses.len());
        for chunk in addresses.chunks(MAX_MULTIPLE_ACCOUNTS) {
            accounts.extend(self.client.get_multiple_accounts(chunk)?);
        }
        Ok(accounts)
    }

    fn program_accounts(
        &self,
        program: &Pubkey,
        filters: &[Filter],
    ) -> Result<Vec<(Pubkey, Account)>> {
        let config = RpcProgramAccountsConfig {
            filters: Some(
                filters
                    .iter()
                    .map(|filter| {
                        RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
                            filter.offset,
                            filter.bytes.clone(),
                        ))
                    })
                    .collect(),
            ),
            account_config: RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                commitment: Some(self.client.commitment()),
                ..RpcAccountInfoConfig::default()
            },
            ..RpcProgramAccountsConfig::default()
        };
        let accounts = self
            .client
            .get_program_ui_accounts_with_config(program, config)
            .map_err(|error| anyhow!("couldn't list accounts of {program}: {error}"))?;
        accounts
            .into_iter()
            .map(|(address, account)| {
                let account = account
                    .decode::<Account>()
                    .with_context(|| format!("couldn't decode account {address}"))?;
                Ok((address, account))
            })
            .collect()
    }

    fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Signature, TxError> {
        let blockhash = self.client.get_latest_blockhash().map_err(tx_error)?;
        let payer = signers[0].pubkey();
        let transaction =
            Transaction::new_signed_with_payer(instructions, Some(&payer), signers, blockhash);
        self.client
            .send_and_confirm_transaction(&transaction)
            .map_err(tx_error)
    }
}

/// Keeps the simulation logs of a failed send, so the program's error can be shown.
fn tx_error(error: ClientError) -> TxError {
    let logs = match error.kind() {
        ClientErrorKind::RpcError(RpcError::RpcResponseError {
            data: RpcResponseErrorData::SendTransactionPreflightFailure(simulation),
            ..
        }) => simulation.logs.clone().unwrap_or_default(),
        _ => Vec::new(),
    };
    TxError {
        message: error.to_string(),
        logs,
    }
}
