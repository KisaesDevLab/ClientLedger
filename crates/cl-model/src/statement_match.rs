//! Matching a vendor statement against the books (P17, plan §3 WP3, D4,
//! Q-P17q).
//!
//! # Handoff (WP0 skeleton, WP3 fills this file)
//!
//! WP3 adds `StatementLine {date, ref, kind: Charge|Credit|Payment,
//! amount}`, `BooksItem {id, kind, date, ref, amount}`, `parse_paste(text)`
//! (tab- or comma-separated; bad rows reported by line number) and
//! `match_lines(stmt, books, date_slack_days) -> MatchResult {pairs,
//! missing_in_books, missing_on_statement, ambiguous}`: exact amounts, never
//! many-to-one, deterministic. No running balance. The screen is `cl-ui`
//! `screens/bill/statement_match.rs` (WP6).
