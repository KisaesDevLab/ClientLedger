//! Attach a document to a transaction already in the books (A04 D3,
//! Q-282, Q-A04e). WP-F2 (step 13) builds it: the candidates from
//! `cl_docread::register_match` (WP-R) shown beside the document's own
//! date, payee, amount and account; focus on the list with nothing
//! selected and Cancel the default button, so Enter never attaches;
//! **Attach to selected** (Alt+T, after a pick) with one confirmation;
//! **Enter as new** (Alt+E) acknowledging the shown ids; then one atomic
//! `DocumentDrafts::attach` (U-95, WP-E). No undo in A04; no learning
//! after an attach.
//!
//! Scaffold (WP-M): nothing here yet.
