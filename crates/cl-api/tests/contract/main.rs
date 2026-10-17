//! Contract tests against the real engine (P05 plan §2.10, §7). One binary,
//! one engine, one fresh cluster; tests run in the order listed below.
//!
//! Run: `cargo xtask engine && cargo xtask pg-fetch`, then
//! `cargo test -p cl-api --test contract` (`CL_API_REQUIRE=1` in CI).

#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

#[path = "../support/mod.rs"]
mod support;

mod common;
mod contract_capabilities;
mod contract_closed_period;
mod contract_companies;
mod contract_conflict;
mod contract_errors;
mod contract_harness;
mod contract_journal;
mod contract_logging;
mod contract_money;
mod contract_resources;
mod contract_session;

use support::{EngineMode, TestFn, run_suite};

fn main() -> std::process::ExitCode {
    let tests: &[(&str, TestFn)] = &[
        (
            "harness::boots_once_and_opens_contract_co",
            contract_harness::boots_once_and_opens_contract_co,
        ),
        (
            "companies::list_excludes_the_workspace_and_create_is_listed",
            contract_companies::list_excludes_the_workspace_and_create_is_listed,
        ),
        (
            "companies::open_close_and_scope_epoch",
            contract_companies::open_close_and_scope_epoch,
        ),
        (
            "companies::profile_round_trip_and_settings",
            contract_companies::profile_round_trip_and_settings,
        ),
        ("accounts::crud_and_filters", contract_resources::accounts),
        ("contacts::crud_and_filters", contract_resources::contacts),
        ("items::crud_and_filters", contract_resources::items),
        ("tags::crud_and_filters", contract_resources::tags),
        (
            "journal::round_trip_create_read_update_void_list",
            contract_journal::round_trip_create_read_update_void_list,
        ),
        (
            "journal::lost_reply_is_reconciled_to_exactly_one_posting",
            contract_journal::lost_reply_is_reconciled_to_exactly_one_posting,
        ),
        (
            "money::seeded_amounts_round_trip_string_equal",
            contract_money::seeded_amounts_round_trip_string_equal,
        ),
        (
            "money::boundaries_round_trip",
            contract_money::boundaries_round_trip,
        ),
        (
            "money::local_refusals_send_nothing",
            contract_money::local_refusals_send_nothing,
        ),
        (
            "money::money_fields_are_strings_on_the_wire",
            contract_money::money_fields_are_strings_on_the_wire,
        ),
        (
            "money::engine_tolerance_probe_money_strict",
            contract_money::engine_tolerance_probe_money_strict,
        ),
        (
            "errors::validation_errors_carry_the_field",
            contract_errors::validation_errors_carry_the_field,
        ),
        (
            "errors::not_found_and_deactivate_with_balance",
            contract_errors::not_found_and_deactivate_with_balance,
        ),
        (
            "errors::tag_conflicts_in_use_and_duplicate",
            contract_errors::tag_conflicts_in_use_and_duplicate,
        ),
        (
            "errors::bad_login_is_invalid_credentials",
            contract_errors::bad_login_is_invalid_credentials,
        ),
        (
            "conflict::two_clients_contact_and_entry",
            contract_conflict::two_clients_contact_and_entry,
        ),
        (
            "conflict::posting_does_not_conflict_an_account_rename",
            contract_conflict::posting_does_not_conflict_an_account_rename,
        ),
        (
            "conflict::server_version_conflict_u17",
            contract_conflict::server_version_conflict_u17,
        ),
        (
            "closed_period::locked_dates_are_refused_both_ways",
            contract_closed_period::locked_dates_are_refused_both_ways,
        ),
        (
            "capabilities::error_codes_u21",
            contract_capabilities::error_codes_u21,
        ),
        (
            "capabilities::idempotency_u18",
            contract_capabilities::idempotency_u18,
        ),
        (
            "capabilities::je_line_contact_u22",
            contract_capabilities::je_line_contact_u22,
        ),
        (
            "capabilities::report_numbers_u14",
            contract_capabilities::report_numbers_u14,
        ),
        (
            "session::refresh_rotates_and_old_cookie_is_refused",
            contract_session::refresh_rotates_and_old_cookie_is_refused,
        ),
        (
            "session::proactive_refresh_when_the_token_expires",
            contract_session::proactive_refresh_when_the_token_expires,
        ),
        (
            "session::concurrent_requests_share_one_refresh",
            contract_session::concurrent_requests_share_one_refresh,
        ),
        (
            "session::reactive_refresh_after_engine_401",
            contract_session::reactive_refresh_after_engine_401,
        ),
        (
            "logging::canary_never_reaches_logs_or_error_text",
            contract_logging::canary_never_reaches_logs_or_error_text,
        ),
        (
            "queue::a_write_through_the_ui_queue",
            contract_logging::a_write_through_the_ui_queue,
        ),
        (
            "session::logout_ends_the_session",
            contract_session::logout_ends_the_session,
        ),
    ];
    run_suite(EngineMode::Normal, tests)
}
