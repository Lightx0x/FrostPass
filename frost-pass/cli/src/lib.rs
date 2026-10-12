//! FrostPass in the terminal: one app for organizers, minters, buyers and scanners.
//!
//! Everything except drawing lives outside [`ui`], so it can be tested against LiteSVM:
//! [`chain`] talks to the cluster, [`model`] loads accounts, [`checks`] holds the program's
//! rules, [`actions`] turns each action into a checked [`actions::Plan`], [`pass`] is the
//! holder's signed door pass, and [`store`] keeps the transaction log and event order on disk.

pub mod actions;
pub mod chain;
pub mod checks;
pub mod format;
pub mod history;
pub mod model;
pub mod pass;
pub mod store;
pub mod ui;
