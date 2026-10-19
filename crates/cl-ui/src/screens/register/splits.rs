//! The Splits panel under the entry row (WP6 step 2 fills it in).

use cl_core::Money;
use cl_core::ids::{AccountId, TagId};
use cl_grid::TextKey;
use cl_model::CommandId;

use super::RegisterScreen;
use super::entry_row::PLine;

#[allow(
    clippy::unused_self,
    reason = "the Splits panel lands in the next step"
)]
impl RegisterScreen {
    pub(super) fn splits_first_account(&self) -> Option<AccountId> {
        None
    }
    pub(super) fn splits_commit(&mut self) -> bool {
        true
    }
    pub(super) fn splits_lines(&self, _amount: Money) -> Option<Result<Vec<PLine>, TextKey>> {
        None
    }
    pub(super) fn splits_dirty(&self) -> bool {
        false
    }
    pub(super) fn close_splits(&mut self) {}
    pub(super) fn toggle_splits(&mut self) {}
    pub(super) fn splits_command(&mut self, _id: CommandId) -> Option<bool> {
        None
    }
    pub(super) fn splits_state(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    pub(super) fn edit_line_memo(&self) -> Option<String> {
        None
    }
    pub(super) fn edit_line_tag(&self) -> Option<TagId> {
        None
    }
    pub(super) fn entry_lists_changed(&mut self) {}
}
