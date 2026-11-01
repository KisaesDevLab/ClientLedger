//! Contract tests against the real engine (P05 plan Â§2.10, Â§7). One binary,
//! one engine, one fresh cluster; tests run in the order listed below.
//!
//! Run: `cargo xtask engine && cargo xtask pg-fetch`, then
//! `cargo test -p cl-api --test contract` (`CL_API_REQUIRE=1` in CI).

#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

#[path = "../support/mod.rs"]
mod support;

mod common;
mod contract_accounts;
mod contract_ap_void;
mod contract_audit;
mod contract_bank_postings;
mod contract_batch;
mod contract_billable;
mod contract_bills;
mod contract_bounced_check;
mod contract_capabilities;
mod contract_census;
mod contract_check_panel;
mod contract_closed_period;
mod contract_companies;
mod contract_conflict;
mod contract_contacts;
mod contract_credit_memos;
mod contract_customer_deposits;
mod contract_customer_statements;
mod contract_daily_sales;
mod contract_deposits;
mod contract_errors;
mod contract_finance_charges;
mod contract_harness;
mod contract_home;
mod contract_idempotency;
mod contract_invoices;
mod contract_items;
mod contract_journal;
mod contract_journal_entries;
mod contract_lifecycle;
mod contract_list_csv;
mod contract_logging;
mod contract_merges;
mod contract_money;
mod contract_money_helpers;
mod contract_pay_bills;
mod contract_pr3;
mod contract_preferences;
mod contract_printing;
mod contract_receive_payment;
mod contract_refunds;
mod contract_register;
mod contract_register_property;
mod contract_resources;
mod contract_run_due;
mod contract_sales_receipts;
mod contract_session;
mod contract_tags;
mod contract_templates;
mod contract_write_check;

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
        ("contacts::projects_u35", contract_contacts::projects_u35),
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
        ("items::sales_kinds_u39a", contract_items::sales_kinds_u39a),
        (
            "invoices::mockup_invoice_totals_3234_87",
            contract_invoices::mockup_invoice_totals_3234_87,
        ),
        (
            "invoices::rounding_golden_cases",
            contract_invoices::rounding_golden_cases,
        ),
        (
            "invoices::price_level_and_discount",
            contract_invoices::price_level_and_discount,
        ),
        ("invoices::unsent_filter", contract_invoices::unsent_filter),
        (
            "invoices::invoice_closed_period",
            contract_invoices::invoice_closed_period,
        ),
        (
            "invoices::invoice_stale_409",
            contract_invoices::invoice_stale_409,
        ),
        (
            "invoices::invoice_put_idempotent",
            contract_invoices::invoice_put_idempotent,
        ),
        (
            "invoices::email_not_configured",
            contract_invoices::email_not_configured,
        ),
        ("invoices::layouts", contract_invoices::layouts),
        (
            "receive_payment::mockup_payment_4000_leaves_385_open",
            contract_receive_payment::mockup_payment_4000_leaves_385_open,
        ),
        (
            "receive_payment::write_off_closes_invoice_to_chosen_account",
            contract_receive_payment::write_off_closes_invoice_to_chosen_account,
        ),
        (
            "receive_payment::overpayment_credit_apply_refund",
            contract_receive_payment::overpayment_credit_apply_refund,
        ),
        (
            "receive_payment::receive_and_apply_retry_same_key_posts_once",
            contract_receive_payment::receive_and_apply_retry_same_key_posts_once,
        ),
        (
            "receive_payment::early_discount_suggested_and_posted",
            contract_receive_payment::early_discount_suggested_and_posted,
        ),
        (
            "receive_payment::void_via_transactions_route_reopens",
            contract_receive_payment::void_via_transactions_route_reopens,
        ),
        (
            "receive_payment::payment_stale_409",
            contract_receive_payment::payment_stale_409,
        ),
        (
            "receive_payment::invoice_balance_changed_409",
            contract_receive_payment::invoice_balance_changed_409,
        ),
        (
            "receive_payment::exact_overapply_refused",
            contract_receive_payment::exact_overapply_refused,
        ),
        (
            "receive_payment::payment_closed_period_password",
            contract_receive_payment::payment_closed_period_password,
        ),
        (
            "receive_payment::payment_cross_company_404",
            contract_receive_payment::payment_cross_company_404,
        ),
        (
            "credit_memos::credit_memo_reduces_ar_exactly",
            contract_credit_memos::credit_memo_reduces_ar_exactly,
        ),
        (
            "credit_memos::credit_memo_keep_then_apply_later",
            contract_credit_memos::credit_memo_keep_then_apply_later,
        ),
        (
            "credit_memos::credit_date_rules",
            contract_credit_memos::credit_date_rules,
        ),
        (
            "credit_memos::credit_memo_stale_409",
            contract_credit_memos::credit_memo_stale_409,
        ),
        (
            "credit_memos::credit_memo_closed_period_password",
            contract_credit_memos::credit_memo_closed_period_password,
        ),
        (
            "credit_memos::credit_memo_cross_company_404",
            contract_credit_memos::credit_memo_cross_company_404,
        ),
        (
            "refunds::refund_clears_credit",
            contract_refunds::refund_clears_credit,
        ),
        (
            "refunds::partial_refund_apply_rest_and_over_refund",
            contract_refunds::partial_refund_apply_rest_and_over_refund,
        ),
        (
            "refunds::refund_check_number_with_print_later_400",
            contract_refunds::refund_check_number_with_print_later_400,
        ),
        (
            "refunds::refund_cross_company_404",
            contract_refunds::refund_cross_company_404,
        ),
        (
            "sales_receipts::sales_receipt_items_tax",
            contract_sales_receipts::sales_receipt_items_tax,
        ),
        (
            "sales_receipts::negative_receipt_needs_bank",
            contract_sales_receipts::negative_receipt_needs_bank,
        ),
        (
            "sales_receipts::sales_receipt_cross_company_404",
            contract_sales_receipts::sales_receipt_cross_company_404,
        ),
        (
            "customer_statements::statement_aging_equals_ar_aging",
            contract_customer_statements::statement_aging_equals_ar_aging,
        ),
        (
            "customer_statements::customer_statement_run_send_history",
            contract_customer_statements::customer_statement_run_send_history,
        ),
        (
            "customer_statements::statement_cross_company_404",
            contract_customer_statements::statement_cross_company_404,
        ),
        (
            "finance_charges::finance_charge_idempotent_per_period",
            contract_finance_charges::finance_charge_idempotent_per_period,
        ),
        (
            "finance_charges::finance_charge_later_month_400",
            contract_finance_charges::finance_charge_later_month_400,
        ),
        (
            "finance_charges::finance_charge_cross_company_404",
            contract_finance_charges::finance_charge_cross_company_404,
        ),
        (
            "finance_charges::finance_charge_client_change_409",
            contract_finance_charges::finance_charge_client_change_409,
        ),
        (
            "pr3::sales_doc_email_no_smtp_and_outbox_kind",
            contract_pr3::sales_doc_email_no_smtp_and_outbox_kind,
        ),
        (
            "pr3::invoice_client_guard_409",
            contract_pr3::invoice_client_guard_409,
        ),
        (
            "customer_deposits::customer_deposit_lifecycle",
            contract_customer_deposits::customer_deposit_lifecycle,
        ),
        (
            "bounced_check::bounced_check_restores_balances",
            contract_bounced_check::bounced_check_restores_balances,
        ),
        (
            "bounced_check::undeposited_bounce_takes_it_from_clearing",
            contract_bounced_check::undeposited_bounce_takes_it_from_clearing,
        ),
        (
            "billable::billable_consumed",
            contract_billable::billable_consumed,
        ),
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
            "merges::closed_period_needs_the_password_u47",
            contract_merges::closed_period_needs_the_password_u47,
        ),
        (
            "tags::required_groups_u42",
            contract_tags::required_groups_u42,
        ),
        (
            "tags::several_line_tags_u45",
            contract_tags::several_line_tags_u45,
        ),
        // P16 step 0e: tag numbers (PENDING until the pin has U-75).
        ("tags::numbers_u75", contract_tags::numbers_u75),
        (
            "tags::number_archive_round_trip_u75",
            contract_tags::number_archive_round_trip_u75,
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
            "check_panel::draft_files_link_onto_a_saved_record_and_list_by_type",
            contract_check_panel::draft_files_link_onto_a_saved_record_and_list_by_type,
        ),
        (
            "check_panel::rules_evaluate",
            contract_check_panel::rules_evaluate,
        ),
        (
            "check_panel::ai_preview_is_refused_while_ai_is_off",
            contract_check_panel::ai_preview_is_refused_while_ai_is_off,
        ),
        ("write_check::gl_effect", contract_write_check::gl_effect),
        ("write_check::edges", contract_write_check::edges),
        (
            "write_check::edit_without_check_edit",
            contract_write_check::edit_without_check_edit,
        ),
        (
            "write_check::printed_check_edits",
            contract_write_check::printed_check_edits,
        ),
        (
            "write_check::closed_period",
            contract_write_check::closed_period,
        ),
        ("daily_sales::over_short", contract_daily_sales::over_short),
        (
            "daily_sales::preview_property",
            contract_daily_sales::preview_property,
        ),
        (
            "daily_sales::closed_period",
            contract_daily_sales::closed_period,
        ),
        // P14 lane A: Make Deposit and Transfer Funds.
        (
            "deposits::grouping_clears_payments_clearing",
            contract_deposits::grouping_clears_payments_clearing,
        ),
        (
            "deposits::fees_and_cash_back",
            contract_deposits::fees_and_cash_back,
        ),
        ("deposits::closed_period", contract_deposits::closed_period),
        ("deposits::vendor_refund", contract_deposits::vendor_refund),
        (
            "deposits::feed_deposit_groupings",
            contract_deposits::feed_deposit_groupings,
        ),
        (
            "deposits::clearing_warning_on_create_and_edit",
            contract_deposits::clearing_warning_on_create_and_edit,
        ),
        // P15 lane J: Make Journal Entry (part A capabilities PENDING).
        (
            "journal_entries::engine_refuses_unbalanced_raw",
            contract_journal_entries::engine_refuses_unbalanced_raw,
        ),
        (
            "journal_entries::exact_balance_no_tolerance",
            contract_journal_entries::exact_balance_no_tolerance,
        ),
        (
            "journal_entries::ar_ap_names",
            contract_journal_entries::ar_ap_names,
        ),
        (
            "journal_entries::numbering",
            contract_journal_entries::numbering,
        ),
        (
            "journal_entries::adjusting_round_trip",
            contract_journal_entries::adjusting_round_trip,
        ),
        (
            "journal_entries::reversing_creates_linked_entry_on_date",
            contract_journal_entries::reversing_creates_linked_entry_on_date,
        ),
        (
            "journal_entries::editing_without_reverse_on_removes_the_reversal",
            contract_journal_entries::editing_without_reverse_on_removes_the_reversal,
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
            "closed_period::moving_out_of_a_closed_period_needs_the_password",
            contract_closed_period::moving_out_of_a_closed_period_needs_the_password,
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
            "idempotency::lost_reply_replays_one_posting",
            contract_idempotency::lost_reply_replays_one_posting,
        ),
        (
            "idempotency::reused_key_is_refused",
            contract_idempotency::reused_key_is_refused,
        ),
        (
            "idempotency::keyed_routes_replay",
            contract_idempotency::keyed_routes_replay,
        ),
        (
            "idempotency::ap_routes_replay",
            contract_idempotency::ap_routes_replay,
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
            "lifecycle::sample_chart_has_one_account_per_equity_role",
            contract_lifecycle::sample_chart_has_one_account_per_equity_role,
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
            "money_helpers::transfers_in_both_registers",
            contract_money_helpers::transfers_in_both_registers,
        ),
        (
            "money_helpers::card_charge_and_credit",
            contract_money_helpers::card_charge_and_credit,
        ),
        (
            "money_helpers::personal_funds_postings",
            contract_money_helpers::personal_funds_postings,
        ),
        (
            "money_helpers::owner_money_postings",
            contract_money_helpers::owner_money_postings,
        ),
        (
            "money_helpers::loan_payments_match_the_lender",
            contract_money_helpers::loan_payments_match_the_lender,
        ),
        // P15 lane T: templates and the catch-up run (PENDING until the pin
        // carries templates-v1 / recurring-run-due-v1).
        (
            "templates::draft_round_trip",
            contract_templates::draft_round_trip,
        ),
        (
            "templates::grouped_deposit_refused",
            contract_templates::grouped_deposit_refused,
        ),
        (
            "templates::stale_on_replace",
            contract_templates::stale_on_replace,
        ),
        (
            "run_due::monthly_catch_up_exactly_once_across_restarts",
            contract_run_due::monthly_catch_up_exactly_once_across_restarts,
        ),
        (
            "run_due::parallel_runs_post_once",
            contract_run_due::parallel_runs_post_once,
        ),
        (
            "run_due::lost_reply_same_run_id",
            contract_run_due::lost_reply_same_run_id,
        ),
        (
            "run_due::closed_period_skipped_then_entered_with_password",
            contract_run_due::closed_period_skipped_then_entered_with_password,
        ),
        (
            "run_due::inactive_account_skipped_no_partial",
            contract_run_due::inactive_account_skipped_no_partial,
        ),
        (
            "run_due::remind_mode_lists_not_posts",
            contract_run_due::remind_mode_lists_not_posts,
        ),
        (
            "run_due::ends_after_count",
            contract_run_due::ends_after_count,
        ),
        (
            "run_due::catch_up_capped_continues_next_call",
            contract_run_due::catch_up_capped_continues_next_call,
        ),
        (
            "run_due::owner_without_permission_skipped",
            contract_run_due::owner_without_permission_skipped,
        ),
        (
            "run_due::journal_template_keeps_names_and_adjusting",
            contract_run_due::journal_template_keeps_names_and_adjusting,
        ),
        (
            "run_due::filter_holds_only_the_asked_template_u82",
            contract_run_due::filter_holds_only_the_asked_template_u82,
        ),
        // P15A lane A: Bulk Entry (PENDING by the batch-*-v1 capabilities;
        // all live from pin c217bfd, engine PR #163).
        (
            "batch::validate_one_section_live",
            contract_batch::validate_one_section_live,
        ),
        (
            "batch::suggest_account_offline",
            contract_batch::suggest_account_offline,
        ),
        ("batch::parse_csv_exact", contract_batch::parse_csv_exact),
        ("batch::fifty_rows_exact", contract_batch::fifty_rows_exact),
        (
            "batch::mixed_one_call_all_in_register",
            contract_batch::mixed_one_call_all_in_register,
        ),
        (
            "batch::check_numbers_unique_and_counter",
            contract_batch::check_numbers_unique_and_counter,
        ),
        (
            "batch::one_bad_row_saves_nothing",
            contract_batch::one_bad_row_saves_nothing,
        ),
        ("batch::replay_same_key", contract_batch::replay_same_key),
        (
            "batch::je_group_unbalanced_refused",
            contract_batch::je_group_unbalanced_refused,
        ),
        (
            "batch::closed_period_row",
            contract_batch::closed_period_row,
        ),
        (
            "batch::replay_other_user_refused",
            contract_batch::replay_other_user_refused,
        ),
        (
            "batch::permission_per_kind",
            contract_batch::permission_per_kind,
        ),
        (
            "batch::row_rules_refused",
            contract_batch::row_rules_refused,
        ),
        // P16 WP6a: Home reads (the dashboard money test is PENDING(U-76)
        // until the pin carries dashboard-exact-v1, engine PR #168).
        (
            "home::bills_count_matches_the_list",
            contract_home::bills_count_matches_the_list,
        ),
        (
            "home::counts_match_the_engine",
            contract_home::counts_match_the_engine,
        ),
        (
            "home::float_dashboard_refused",
            contract_home::float_dashboard_refused,
        ),
        ("home::exact_strings_u76", contract_home::exact_strings_u76),
        // P16 WP7: the rail counts Home reads.
        (
            "home::rail_counts_match_the_lists",
            contract_home::rail_counts_match_the_lists,
        ),
        // P17 WP2b: the register's A/P void guard (review B1).
        (
            "ap_void::paid_bill_void_is_refused_and_links_remain",
            contract_ap_void::paid_bill_void_is_refused_and_links_remain,
        ),
        // P17 WP2: Enter Bill and vendor credits (PR-A/PR-B parts PENDING
        // by capability until the pin moves).
        (
            "bills::expense_bill_posts_to_ap_live",
            contract_bills::expense_bill_posts_to_ap_live,
        ),
        (
            "bills::vendor_summary_moves_by_the_total_live",
            contract_bills::vendor_summary_moves_by_the_total_live,
        ),
        (
            "bills::vendor_statement_balance_and_kept_file_live",
            contract_bills::vendor_statement_balance_and_kept_file_live,
        ),
        (
            "bills::void_refused_when_paid_live",
            contract_bills::void_refused_when_paid_live,
        ),
        (
            "bills::void_bill_drops_from_summary_u84",
            contract_bills::void_bill_drops_from_summary_u84,
        ),
        (
            "vendor_credits::create_and_available_live",
            contract_bills::vendor_credit_create_and_available_live,
        ),
        (
            "bills::attach_draft_live",
            contract_bills::attach_draft_live,
        ),
        (
            "bills::attachments_company_u90_live",
            contract_bills::attachments_company_u90_live,
        ),
        (
            "bills::attachment_10mb_edge_live",
            contract_bills::attachment_10mb_edge_live,
        ),
        (
            "bills::reading_refused_while_ai_is_off_live",
            contract_bills::reading_refused_while_ai_is_off_live,
        ),
        (
            "bills::closed_period_u83",
            contract_bills::closed_period_u83,
        ),
        ("bills::stale_u84", contract_bills::stale_u84),
        ("bills::delete_u84", contract_bills::delete_u84),
        (
            "vendor_credits::edit_u84",
            contract_bills::vendor_credit_edit_u84,
        ),
        ("bills::item_lines_u85", contract_bills::item_lines_u85),
        ("bills::terms_dates_u86", contract_bills::terms_dates_u86),
        ("bills::received_u87", contract_bills::received_u87),
        (
            "pay_bills::partial_payment_with_credit_live",
            contract_pay_bills::partial_payment_with_credit_live,
        ),
        (
            "pay_bills::card_payment_live",
            contract_pay_bills::card_payment_live,
        ),
        (
            "pay_bills::mockup_run_with_discount_u101",
            contract_pay_bills::mockup_run_with_discount_u101,
        ),
        (
            "pay_bills::credit_and_discount_without_cash_u101",
            contract_pay_bills::credit_and_discount_without_cash_u101,
        ),
        (
            "pay_bills::keyed_pay_u102",
            contract_pay_bills::keyed_pay_u102,
        ),
        (
            "pay_bills::check_numbers_now_u103",
            contract_pay_bills::check_numbers_now_u103,
        ),
        (
            "printing::check_print_cycle",
            contract_printing::check_print_cycle,
        ),
        (
            "printing::reprint_closed_period",
            contract_printing::reprint_closed_period,
        ),
        (
            "printing::logo_and_footers",
            contract_printing::logo_and_footers,
        ),
        (
            "printing::document_numbers_live",
            contract_printing::document_numbers_live,
        ),
        (
            "printing::forms_queue_live",
            contract_printing::forms_queue_live,
        ),
        (
            "pay_bills::voided_payment_leaves_the_queue_u115",
            contract_pay_bills::voided_payment_leaves_the_queue_u115,
        ),
        (
            "pay_bills::credits_only_queues_no_check_u117",
            contract_pay_bills::credits_only_queues_no_check_u117,
        ),
        (
            "pay_bills::void_with_cash_credit_discount_restores_ap_u117",
            contract_pay_bills::void_with_cash_credit_discount_restores_ap_u117,
        ),
        (
            "printing::voided_check_leaves_the_queue_u115",
            contract_printing::voided_check_leaves_the_queue_u115,
        ),
        (
            "printing::voided_checks_read_back",
            contract_printing::voided_checks_read_back,
        ),
        (
            "printing::keyed_print_replays_u116",
            contract_printing::keyed_print_replays_u116,
        ),
        (
            "printing::requeue_skips_void_u117",
            contract_printing::requeue_skips_void_u117,
        ),
        (
            "bills::backup_restore_keeps_the_bill_file_live",
            contract_bills::backup_restore_keeps_the_bill_file_live,
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
