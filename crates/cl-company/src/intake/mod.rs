//! Intake folders (A01): the folders a company's bills and receipts are
//! saved into, watched so each new file is copied into the company.
//!
//! B1 only moves the P17 receipt-inbox code here; the rules of A01 follow.

pub mod scan;

pub use scan::*;
