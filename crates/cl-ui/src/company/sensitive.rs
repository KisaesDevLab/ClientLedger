//! Moves the sensitive marks kept on this computer into the engine (P29,
//! U-33, plan §2 "Moving the local marks").
//!
//! Before `sensitive-accounts-v1` the Chart of Accounts kept the marks in
//! the company's local preferences (Q-157). On company open, in an Admin
//! session only, each one is sent once with `PUT /accounts/{id}/sensitive`:
//!
//! - marked (200) or the account is gone (404): the local mark is dropped;
//! - refused (422 `SENSITIVE_NOT_ALLOWED`: a bank, card or built-in
//!   account): the mark stays local and the Chart of Accounts says so;
//! - anything else (offline engine, a timeout): it stays for the next open.
//!
//! It is idempotent, so a restore of an older backup (whose preferences
//! carry the old marks) simply moves them again. A non-Admin open does
//! nothing. No account name or id is logged, only counts.

use std::collections::BTreeSet;
use std::sync::Arc;

use cl_api::{ApiError, IdempotencyKey, OrphanLabel, Session};
use cl_core::ids::{AccountId, TenantId};

use super::CompanyLink;

/// Something that marks one account sensitive (the session; a fake in
/// tests).
pub trait Marker {
    /// Marks `id` sensitive.
    fn mark(&self, id: AccountId)
    -> impl std::future::Future<Output = Result<(), ApiError>> + Send;
}

impl Marker for Session {
    async fn mark(&self, id: AccountId) -> Result<(), ApiError> {
        // A one-time move of a local mark at open: the account is read for
        // its stamp just before (U-291; LAN mode requires one, U-292). The
        // mark is idempotent and changes no other field.
        let read = self.accounts().get(id).await?;
        self.accounts()
            .set_sensitive(id, &read.version, true, IdempotencyKey::new())
            .await
            .map(|_| ())
    }
}

/// What happened to each local mark.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MoveOutcome {
    /// Marked on the engine, or the account is gone: drop locally.
    pub moved: BTreeSet<AccountId>,
    /// The engine refused (bank, card or built-in): keep, with a note.
    pub refused: BTreeSet<AccountId>,
    /// Not answered (try again at the next open): keep.
    pub kept: BTreeSet<AccountId>,
}

/// True when this open should move the marks: an Admin, an engine that
/// stores them, and something to move. Never in the demo.
#[must_use]
pub fn should_move(demo: bool, admin: bool, engine_stores: bool, local: usize) -> bool {
    !demo && admin && engine_stores && local > 0
}

/// Sends each mark, one at a time (a partial failure keeps the rest).
pub async fn move_marks(m: &impl Marker, ids: &BTreeSet<AccountId>) -> MoveOutcome {
    let mut out = MoveOutcome::default();
    for id in ids {
        match m.mark(*id).await {
            Ok(()) | Err(ApiError::NotFound { .. }) => {
                out.moved.insert(*id);
            }
            Err(e) if cl_api::is_sensitive_not_allowed(&e) => {
                out.refused.insert(*id);
            }
            Err(_) => {
                out.kept.insert(*id);
            }
        }
    }
    out
}

/// The local set after a move: only the marks not moved stay.
#[must_use]
pub fn remaining(local: &BTreeSet<AccountId>, out: &MoveOutcome) -> BTreeSet<AccountId> {
    local.difference(&out.moved).copied().collect()
}

/// Starts the move for the company just opened (the shell calls it on
/// `Opened`). Fire and forget: a write on the queue, never cancelled.
pub fn start(link: &Arc<CompanyLink>, tenant: TenantId) {
    let engine_stores = link
        .capabilities()
        .has(cl_api::Capabilities::ACCOUNT_SENSITIVE);
    let Some(local) = link
        .store()
        .load()
        .ok()
        .and_then(|l| l.get(tenant).map(|r| r.prefs.sensitive_accounts.clone()))
    else {
        return;
    };
    if !should_move(
        link.is_demo(),
        link.rights().is_admin(),
        engine_stores,
        local.len(),
    ) {
        return;
    }
    let (Some(session), Some(Ok(group))) = (link.session(), link.group()) else {
        return;
    };
    let store = link.store().clone();
    let label = OrphanLabel {
        what: "orphan.account_save",
        key: None,
    };
    let started = group.write(&session, label, move |s| async move {
        let out = move_marks(&s, &local).await;
        if !out.moved.is_empty()
            && store
                .update(|l| {
                    if let Some(r) = l.get_mut(tenant) {
                        r.prefs.sensitive_accounts = remaining(&r.prefs.sensitive_accounts, &out);
                    }
                })
                .is_err()
        {
            tracing::warn!("moved sensitive marks could not be dropped from this computer");
        }
        tracing::info!(
            moved = out.moved.len(),
            refused = out.refused.len(),
            kept = out.kept.len(),
            "sensitive marks moved to the engine"
        );
        Ok(())
    });
    if started.is_err() {
        tracing::warn!("the sensitive marks could not be moved now; next open tries again");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    fn id(n: u32) -> AccountId {
        AccountId::parse(&format!("a0000000-0000-4000-8000-{n:012x}")).expect("id")
    }

    /// Answers by id; records what was sent.
    struct Fake {
        answers: BTreeMap<AccountId, Result<(), ApiError>>,
        sent: Mutex<Vec<AccountId>>,
    }

    impl Marker for Fake {
        fn mark(
            &self,
            id: AccountId,
        ) -> impl std::future::Future<Output = Result<(), ApiError>> + Send {
            if let Ok(mut s) = self.sent.lock() {
                s.push(id);
            }
            let r = self.answers.get(&id).cloned().unwrap_or(Ok(()));
            async move { r }
        }
    }

    fn not_allowed() -> ApiError {
        cl_api::engine_refusal(
            422,
            &serde_json::json!({"error": {"code": "SENSITIVE_NOT_ALLOWED", "message": "A bank account can not be marked sensitive"}}),
        )
    }

    fn run(f: &Fake, ids: &BTreeSet<AccountId>) -> MoveOutcome {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(move_marks(f, ids))
    }

    #[test]
    fn marked_and_gone_accounts_leave_this_computer_refused_ones_stay() {
        let ids: BTreeSet<_> = [id(1), id(2), id(3), id(4)].into();
        let f = Fake {
            answers: [
                (id(2), Err(ApiError::NotFound { code: None })),
                (id(3), Err(not_allowed())),
                (id(4), Err(ApiError::Timeout)),
            ]
            .into(),
            sent: Mutex::new(Vec::new()),
        };
        let out = run(&f, &ids);
        assert_eq!(out.moved, [id(1), id(2)].into());
        assert_eq!(out.refused, [id(3)].into());
        assert_eq!(out.kept, [id(4)].into(), "a partial failure keeps the rest");
        assert_eq!(f.sent.lock().map_or(0, |s| s.len()), 4);
        assert_eq!(remaining(&ids, &out), [id(3), id(4)].into());
    }

    #[test]
    fn a_second_run_after_a_restore_moves_them_again() {
        // A restored backup brings the old marks back: the engine answers
        // 200 again (idempotent) and they leave once more.
        let ids: BTreeSet<_> = [id(7)].into();
        let f = Fake {
            answers: BTreeMap::new(),
            sent: Mutex::new(Vec::new()),
        };
        let first = run(&f, &ids);
        let second = run(&f, &ids);
        assert_eq!(first, second);
        assert!(remaining(&ids, &second).is_empty());
    }

    #[test]
    fn only_an_admin_on_an_engine_that_stores_marks_moves_them() {
        assert!(should_move(false, true, true, 2));
        assert!(!should_move(false, false, true, 2), "a non-Admin open");
        assert!(!should_move(false, true, false, 2), "an older engine");
        assert!(!should_move(false, true, true, 0), "nothing to move");
        assert!(!should_move(true, true, true, 2), "the demo");
    }
}
