//! FrostPass in the terminal: one app for organizers, minters, buyers and scanners.
//!
//! Everything except drawing lives outside [`ui`], so it can be tested against LiteSVM:
//! [`chain`] talks to the cluster, [`model`] loads accounts, [`checks`] holds the program's
//! rules, [`actions`] turns each action into a checked [`actions::Plan`], and [`pass`] is the
//! holder's signed door pass.

pub mod actions;
pub mod chain;
pub mod checks;
pub mod format;
pub mod model;
pub mod pass;
pub mod ui;
