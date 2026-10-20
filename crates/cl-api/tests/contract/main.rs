//! Contract tests against the real engine (P05 plan §2.10, §7). One binary,
//! one engine, one fresh cluster; tests run in the order listed below.
//!
//! Run: `cargo xtask engine && cargo xtask pg-fetch`, then
//! `cargo test -p cl-api --test contract` (`CL_API_REQUIRE=1` in CI).

#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

#[path = "../support/mod.rs"]
mod support;

mod common;
mod contract_accounts;
mod contract_audit;
mod contract_bank_postings;
mod contract_capabilities;
mod contract_census;
mod contract_check_panel;
mod contract_closed_period;
mod contract_companies;
mod contract_conflict;
mod contract_contacts;
mod contract_errors;
mod contract_harness;
mod contract_items;
mod contract_journal;
mod contract_lifecycle;
mod contract_list_csv;
mod contract_logging;
mod contract_money;
mod contract_preferences;
mod contract_register;
mod contract_register_property;
mod contract_resources;
mod contract_session;
mod contract_tags;

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
        (
            "accounts::unique_numbers",
            contract_accounts::unique_numbers,
        ),
        (
            "accounts::deactivate_and_reactivate",
            contract_accounts::deactivate_and_reactivate,
        ),
        (
            "accounts::own_balance_only",
            contract_accounts::own_balance_only,
        ),
        ("accounts::quick_report", contract_accounts::quick_report),
        ("accounts::merge_u31", contract_accounts::merge_u31),
        (
            "accounts::parent_rules_u32",
            contract_accounts::parent_rules_u32,
        ),
        (
            "accounts::sensitive_flag_u33",
            contract_accounts::sensitive_flag_u33,
        ),
        (
            "accounts::masked_number_u34",
            contract_accounts::masked_number_u34,
        ),
        ("contacts::crud_and_filters", contract_resources::contacts),
        ("contacts::both_roles", contract_contacts::both_roles),
        (
            "contacts::default_expense_account",
            contract_contacts::default_expense_account,
        ),
        (
            "contacts::history_balance_and_merge",
            contract_contacts::history_balance_and_merge,
        ),
        (
            "contacts::client_open_balance",
            contract_contacts::client_open_balance,
        ),
        (
            "contacts::vendor_1099_profile",
            contract_contacts::vendor_1099_profile,
        ),
        ("contacts::jobs_u35", contract_contacts::jobs_u35),
        ("contacts::addresses_u36", contract_contacts::addresses_u36),
        ("contacts::notes_u37", contract_contacts::notes_u37),
        (
            "contacts::open_balance_u38",
            contract_contacts::open_balance_u38,
        ),
        (
            "contacts::w9_on_file_u44",
            contract_contacts::w9_on_file_u44,
        ),
        ("items::crud_and_filters", contract_resources::items),
        ("tags::crud_and_filters", contract_resources::tags),
        (
            "items::reactivate_and_unique_names",
            contract_items::reactivate_and_unique_names,
        ),
        ("items::item_types_u39", contract_items::item_types_u39),
        ("items::price_levels_u40", contract_items::price_levels_u40),
        ("tags::groups_round_trip", contract_tags::groups_round_trip),
        (
            "tags::rename_usage_and_header_merge",
            contract_tags::rename_usage_and_header_merge,
        ),
        (
            "tags::line_tag_merge_u41",
            contract_tags::line_tag_merge_u41,
        ),
        ("tags::group_reorder_u41", contract_tags::group_reorder_u41),
        ("tags::single_select_u41", contract_tags::single_select_u41),
        (
            "tags::required_groups_u42",
            contract_tags::required_groups_u42,
        ),
        (
            "tags::several_line_tags_u45",
            contract_tags::several_line_tags_u45,
        ),
        (
            "list_csv::exports_and_imports",
            contract_list_csv::exports_and_imports,
        ),
        (
            "list_csv::items_import_u46",
            contract_list_csv::items_import_u46,
        ),
        (
            "list_csv::terms_lists_u43",
            contract_list_csv::terms_lists_u43,
        ),
        (
            "census::live_spec_matches_fixtures",
            contract_census::live_spec_matches_fixtures,
        ),
        ("census::field_census", contract_census::field_census),
        (
            "journal::round_trip_create_read_update_void_list",
            contract_journal::round_trip_create_read_update_void_list,
        ),
        (
            "journal::lost_reply_is_reconciled_to_exactly_one_posting",
            contract_journal::lost_reply_is_reconciled_to_exactly_one_posting,
        ),
        (
            "register::dates_paging_and_exact_money",
            contract_register::dates_paging_and_exact_money,
        ),
        (
            "register::card_register_columns_and_sign",
            contract_register::card_register_columns_and_sign,
        ),
        (
            "bank_postings::each_kind_round_trip_update_and_void",
            contract_bank_postings::each_kind_round_trip_update_and_void,
        ),
        (
            "bank_postings::lost_replies_are_reconciled_to_one_posting",
            contract_bank_postings::lost_replies_are_reconciled_to_one_posting,
        ),
        (
            "check_panel::panel_readers_after_a_bill",
            contract_check_panel::panel_readers_after_a_bill,
        ),
        (
            "check_panel::recent_checks_carry_the_number",
            contract_check_panel::recent_checks_carry_the_number,
        ),
        (
            "check_panel::draft_attachment_moves_onto_the_check",
            contract_check_panel::draft_attachment_moves_onto_the_check,
        ),
        (
            "check_panel::rules_evaluate",
            contract_check_panel::rules_evaluate,
        ),
        (
            "check_panel::ai_preview_is_refused_while_ai_is_off",
            contract_check_panel::ai_preview_is_refused_while_ai_is_off,
        ),
        (
            "register_property::balance_holds_after_random_edits",
            contract_register_property::balance_holds_after_random_edits,
        ),
        (
            "audit::update_and_void_keep_the_before_image",
            contract_audit::update_and_void_keep_the_before_image,
        ),
        (
            "audit::delete_keeps_every_line_in_the_audit_trail",
            contract_audit::delete_keeps_every_line_in_the_audit_trail,
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
            "closed_period::bank_postings_in_a_closed_period",
            contract_closed_period::bank_postings_in_a_closed_period,
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
            "lifecycle::coa_import_settings_and_setup",
            contract_lifecycle::coa_import_settings_and_setup,
        ),
        (
            "lifecycle::opening_balances_and_review",
            contract_lifecycle::opening_balances_and_review,
        ),
        (
            "lifecycle::year_boundary_retained_earnings",
            contract_lifecycle::year_boundary_retained_earnings,
        ),
        (
            "lifecycle::closing_password_u15",
            contract_lifecycle::closing_password_u15,
        ),
        (
            "lifecycle::tenants_list_create_rename_e5a",
            contract_lifecycle::tenants_list_create_rename_e5a,
        ),
        (
            "lifecycle::archive_export_import_e5a",
            contract_lifecycle::archive_export_import_e5a,
        ),
        (
            "lifecycle::safe_delete_e5b",
            contract_lifecycle::safe_delete_e5b,
        ),
        (
            "lifecycle::recovery_key_u23",
            contract_lifecycle::recovery_key_u23,
        ),
        (
            "preferences::ai_config_disclosure_consent_and_tasks",
            contract_preferences::ai_config_disclosure_consent_and_tasks,
        ),
        (
            "preferences::provider_test_ollama_stub_and_keyless_cloud",
            contract_preferences::provider_test_ollama_stub_and_keyless_cloud,
        ),
        (
            "preferences::smtp_settings_round_trip_and_test",
            contract_preferences::smtp_settings_round_trip_and_test,
        ),
        (
            "preferences::session_secrets_e6",
            contract_preferences::session_secrets_e6,
        ),
        (
            "preferences::usage_summary_e6",
            contract_preferences::usage_summary_e6,
        ),
        (
            "preferences::smtp_session_u13",
            contract_preferences::smtp_session_u13,
        ),
        (
            "preferences::token_cap_u29",
            contract_preferences::token_cap_u29,
        ),
        (
            "lifecycle::ten_switches_under_two_seconds",
            contract_lifecycle::ten_switches_under_two_seconds,
        ),
        (
            "session::logout_ends_the_session",
            contract_session::logout_ends_the_session,
        ),
    ];
    run_suite(EngineMode::Normal, tests)
}
