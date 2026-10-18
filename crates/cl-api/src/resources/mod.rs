//! Engine resources and their shared machinery.

pub mod accounts;
pub mod ai;
pub(crate) mod common;
pub mod company;
pub mod contacts;
pub mod export;
pub mod form1099;
pub mod items;
pub mod ledger;
pub mod list_csv;
pub mod operations;
pub mod posting;
pub(crate) mod reconcile;
pub mod recovery;
pub mod reports;
pub mod sales;
pub mod session_secrets;
pub mod setup;
pub mod smtp;
pub mod system;
pub mod tags;
pub mod tenants;
pub mod transactions;
pub mod versioning;

#[cfg(test)]
mod journal_tests;
#[cfg(test)]
pub(crate) mod testkit;
