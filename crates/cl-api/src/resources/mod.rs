//! Engine resources and their shared machinery.

pub mod accounts;
pub mod ai;
pub mod ap_void;
pub mod attachments;
pub mod audit;
pub mod bank_postings;
pub mod batch;
pub mod bills;
pub mod checks;
pub(crate) mod common;
pub mod company;
pub mod contacts;
pub mod daily_sales;
pub mod dashboard;
pub mod export;
pub mod feed;
pub mod form1099;
pub mod home_queries;
pub mod items;
pub mod ledger;
pub mod list_csv;
pub mod operations;
pub mod payments;
pub mod posting;
pub(crate) mod reconcile;
pub mod recovery;
pub mod recurring;
pub mod register;
pub mod reports;
pub mod rules;
pub mod sales;
pub mod session_secrets;
pub mod setup;
pub mod smtp;
pub mod system;
pub mod tags;
pub mod templates;
pub mod tenants;
pub mod transactions;
pub mod versioning;

#[cfg(test)]
mod journal_tests;
#[cfg(test)]
pub(crate) mod testkit;
