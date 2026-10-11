//! `frostpass`: the FrostPass terminal app.
//!
//! ```text
//! frostpass                          # devnet, wallet at ~/.config/solana/id.json
//! frostpass --keypair scanner.json   # act as another wallet (e.g. a door scanner)
//! frostpass --url localnet
//! ```
//!
//! The transaction log and the event order are kept in `~/.local/share/frostpass/<cluster>/`
//! (`$FROSTPASS_DATA` overrides the folder).

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use frost_pass_cli::{chain::RpcChain, store, ui};
use solana_keypair::read_keypair_file;

#[derive(Parser)]
#[command(
    version,
    about = "FrostPass for organizers, minters, buyers and scanners",
    long_about = "FrostPass in the terminal. One tab per actor: organizers create and run events, \
                  minters buy tickets, buyers trade listings, and scanners redeem tickets at the \
                  door with the holder's signed pass. Every action is checked against the \
                  program's rules and shown for confirmation before it's sent."
)]
struct Args {
    /// RPC URL, or `devnet` / `localnet`
    #[arg(short, long, default_value = "devnet")]
    url: String,

    /// Wallet keypair file [default: ~/.config/solana/id.json]
    #[arg(short, long)]
    keypair: Option<PathBuf>,
}

fn rpc_url(url: &str) -> String {
    match url {
        "devnet" | "d" => "https://api.devnet.solana.com".into(),
        "localnet" | "localhost" | "l" => "http://127.0.0.1:8899".into(),
        url => url.into(),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let keypair_path = match args.keypair {
        Some(path) => path,
        None => PathBuf::from(std::env::var("HOME").context("HOME isn't set")?)
            .join(".config/solana/id.json"),
    };
    let wallet = read_keypair_file(&keypair_path).map_err(|error| {
        anyhow!(
            "couldn't read the keypair at {}: {error}",
            keypair_path.display()
        )
    })?;

    let url = rpc_url(&args.url);
    let cluster = match args.url.as_str() {
        "devnet" | "d" => "devnet".to_string(),
        "localnet" | "localhost" | "l" => "localnet".to_string(),
        _ => url.clone(),
    };
    let data_dir = store::data_dir(&cluster);
    ui::run(RpcChain::new(&url), wallet, cluster, data_dir.as_deref())
}
