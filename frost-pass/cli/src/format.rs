//! Formatting and parsing of amounts, rates, keys and times, for display and form input.

use anchor_lang::prelude::Pubkey;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};

/// USDC has 6 decimals: 1 USDC = 1_000_000 base units.
const USDC_DECIMALS: usize = 6;
const USDC_UNIT: u64 = 1_000_000;
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// `10_250_000` → `"10.25"`, `11_000` → `"0.011"`. Always at least two decimals.
pub fn usdc(amount: u64) -> String {
    let whole = amount / USDC_UNIT;
    let fraction = format!("{:06}", amount % USDC_UNIT);
    let fraction = fraction.trim_end_matches('0');
    format!("{whole}.{fraction:0<2}")
}

/// Parses a USDC amount such as `"10"`, `"10.5"` or `"0.011"` into base units.
pub fn parse_usdc(input: &str) -> Result<u64> {
    let input = input.trim().trim_end_matches("USDC").trim();
    if input.is_empty() {
        bail!("enter an amount");
    }
    let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
    if fraction.len() > USDC_DECIMALS {
        bail!("USDC has at most {USDC_DECIMALS} decimals");
    }
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || !(fraction.is_empty() || digits(fraction)) {
        bail!("'{input}' is not an amount");
    }
    let whole: u64 = whole.parse().context("amount too large")?;
    let fraction: u64 = format!("{fraction:0<USDC_DECIMALS$}").parse()?;
    whole
        .checked_mul(USDC_UNIT)
        .and_then(|units| units.checked_add(fraction))
        .context("amount too large")
}

pub fn sol(lamports: u64) -> String {
    format!("{:.4}", lamports as f64 / LAMPORTS_PER_SOL as f64)
}

/// Every lamport, without trailing zeros: `20_000` → `"0.00002"`, `1_500_000_000` → `"1.5"`.
pub fn sol_exact(lamports: u64) -> String {
    let whole = lamports / LAMPORTS_PER_SOL;
    let fraction = lamports % LAMPORTS_PER_SOL;
    if fraction == 0 {
        return whole.to_string();
    }
    let digits = format!("{fraction:09}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

/// `500` → `"5%"`, `1_050` → `"10.5%"`.
pub fn percent(bps: u16) -> String {
    let whole = bps / 100;
    match bps % 100 {
        0 => format!("{whole}%"),
        rest if rest % 10 == 0 => format!("{whole}.{}%", rest / 10),
        rest => format!("{whole}.{rest:02}%"),
    }
}

/// Parses `"5"`, `"5%"` or `"10.5"` (percent, at most two decimals) into basis points.
pub fn parse_percent(input: &str) -> Result<u16> {
    let input = input.trim().trim_end_matches('%').trim();
    let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
    if whole.is_empty() || fraction.len() > 2 {
        bail!("enter a percentage with at most two decimals, e.g. 5 or 10.5");
    }
    let whole: u32 = whole.parse().context("not a percentage")?;
    let fraction: u32 = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<2}")
            .parse()
            .context("not a percentage")?
    };
    let bps = whole
        .checked_mul(100)
        .and_then(|b| b.checked_add(fraction))
        .context("percentage too large")?;
    u16::try_from(bps).context("percentage too large")
}

/// `FZFZLbrBExacfhFE6K9uUKjvxT1HEuvtsqXTP9pMkvRb` → `"FZFZ…kvRb"`.
pub fn short(key: &Pubkey) -> String {
    let text = key.to_string();
    format!("{}…{}", &text[..4], &text[text.len() - 4..])
}

pub fn parse_pubkey(input: &str) -> Result<Pubkey> {
    input
        .trim()
        .parse()
        .with_context(|| format!("'{}' is not a valid address", input.trim()))
}

/// A comma- or space-separated list of addresses.
pub fn parse_pubkeys(input: &str) -> Result<Vec<Pubkey>> {
    input
        .split([',', ' ', '\n'])
        .filter(|part| !part.trim().is_empty())
        .map(parse_pubkey)
        .collect()
}

/// `"2026-10-12 18:00 UTC"`.
/// Search: every word of `query` appears in `text`, ignoring case and order
/// (`"tech light"` finds `"Light Tech Meetup"`).
pub fn matches_words(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

pub fn datetime(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| unix.to_string())
}

/// `"in 3h 12m"` or `"2d 4h ago"`, relative to `now` (cluster time).
pub fn relative(unix: i64, now: i64) -> String {
    let delta = unix - now;
    let span = delta.unsigned_abs();
    let (days, hours, minutes, seconds) = (
        span / 86_400,
        span % 86_400 / 3_600,
        span % 3_600 / 60,
        span % 60,
    );
    let text = match (days, hours, minutes) {
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    };
    if delta >= 0 {
        format!("in {text}")
    } else {
        format!("{text} ago")
    }
}

/// A time relative to `now` (`"+90m"`, `"+2h"`, `"+3d"`) or an absolute UTC time
/// (`"2026-10-12 18:00"`).
pub fn parse_time(input: &str, now: i64) -> Result<i64> {
    let input = input.trim();
    if let Some(offset) = input.strip_prefix('+') {
        let (number, unit) = offset.split_at(offset.len().saturating_sub(1));
        let number: i64 = number
            .parse()
            .with_context(|| format!("'{input}' is not a duration like +90m, +2h or +3d"))?;
        let seconds = match unit {
            "m" => 60,
            "h" => 3_600,
            "d" => 86_400,
            _ => bail!("'{input}': use m, h or d, e.g. +90m, +2h or +3d"),
        };
        return number
            .checked_mul(seconds)
            .and_then(|offset| now.checked_add(offset))
            .context("time too far away");
    }
    let time = NaiveDateTime::parse_from_str(input, "%Y-%m-%d %H:%M").with_context(|| {
        format!("'{input}' is not a time: use +2h, +3d or YYYY-MM-DD HH:MM (UTC)")
    })?;
    Ok(time.and_utc().timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sol_exact_keeps_every_lamport() {
        assert_eq!(sol_exact(0), "0");
        assert_eq!(sol_exact(20_000), "0.00002");
        assert_eq!(sol_exact(1_500_000_000), "1.5");
        assert_eq!(sol_exact(12_539_280), "0.01253928");
    }

    #[test]
    fn amounts_rates_and_times_round_trip() {
        assert_eq!(usdc(10_250_000), "10.25");
        assert_eq!(usdc(11_000), "0.011");
        assert_eq!(usdc(0), "0.00");
        assert_eq!(parse_usdc("10.25").unwrap(), 10_250_000);
        assert_eq!(parse_usdc("0.011 USDC").unwrap(), 11_000);
        assert!(parse_usdc("1.0000001").is_err());
        assert!(parse_usdc("-1").is_err());
        assert!(parse_usdc("1e3").is_err());

        assert_eq!(percent(500), "5%");
        assert_eq!(percent(1_050), "10.5%");
        assert_eq!(parse_percent("10.5%").unwrap(), 1_050);
        assert!(parse_percent("700").is_err());

        assert_eq!(parse_time("+2h", 1_000).unwrap(), 1_000 + 7_200);
        assert_eq!(parse_time("2026-10-12 18:00", 0).unwrap(), 1_791_828_000);
        assert_eq!(datetime(1_791_828_000), "2026-10-12 18:00 UTC");
        assert!(parse_time("+2w", 0).is_err());
        assert_eq!(relative(1_000 + 3_660, 1_000), "in 1h 1m");
        assert_eq!(relative(1_000 - 172_800, 1_000), "2d 0h ago");
    }
}
