//! The owner's equity and pay-back account choices, remembered per company
//! (Q-P14m): without `equity-roles-v1` the owner picks the account once and
//! the helpers start on it next time. One small capped file next to the
//! company list, holding a hashed company key, a slot name and an account
//! id per line (no names or amounts).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cl_core::ids::AccountId;

/// The file, next to the company list.
const FILE: &str = "owner-accounts.txt";
/// Largest file read (anything bigger is ignored).
const MAX_READ: u64 = 64 * 1024;
/// Most entries kept.
const MAX_ENTRIES: usize = 512;

/// What a remembered account is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Pay yourself (draws, distributions).
    Out,
    /// Put money in (contributions, capital).
    In,
    /// Paid with personal funds, "The business will pay me back".
    PayBack,
}

impl Slot {
    fn name(self) -> &'static str {
        match self {
            Slot::Out => "out",
            Slot::In => "in",
            Slot::PayBack => "payback",
        }
    }
}

/// Where the file lives for this link (`None` without a settings folder).
pub(crate) fn dir_of(link: &crate::company::CompanyLink) -> Option<PathBuf> {
    link.store().path().parent().map(Path::to_path_buf)
}

fn key(company: &str, slot: Slot) -> String {
    format!("{}/{}", crate::prefs::key_hash(company), slot.name())
}

fn load(dir: &Path) -> BTreeMap<String, AccountId> {
    let path = dir.join(FILE);
    let small = std::fs::metadata(&path).is_ok_and(|m| m.len() <= MAX_READ);
    if !small {
        return BTreeMap::new();
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(' ')?;
            Some((k.to_string(), AccountId::parse(v.trim()).ok()?))
        })
        .take(MAX_ENTRIES)
        .collect()
}

/// The remembered account for `company` and `slot`.
pub(crate) fn get(dir: Option<&Path>, company: &str, slot: Slot) -> Option<AccountId> {
    load(dir?).get(&key(company, slot)).copied()
}

/// Remembers `account` for `company` and `slot` (best effort: a failed
/// write only means the owner picks again next time).
pub(crate) fn set(dir: Option<&Path>, company: &str, slot: Slot, account: AccountId) {
    let Some(dir) = dir else { return };
    let mut book = load(dir);
    book.insert(key(company, slot), account);
    while book.len() > MAX_ENTRIES {
        let Some(first) = book.keys().next().cloned() else {
            break;
        };
        book.remove(&first);
    }
    let mut text = String::new();
    for (k, v) in &book {
        text.push_str(k);
        text.push(' ');
        text.push_str(&v.to_string());
        text.push('\n');
    }
    let tmp = dir.join(format!("{FILE}.{}.tmp", std::process::id()));
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, text))
        .and_then(|()| std::fs::rename(&tmp, dir.join(FILE)));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        tracing::debug!("the owner account choice was not written");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_per_company_and_slot() {
        let dir = tempfile::tempdir().expect("tmp");
        let a = AccountId::parse("00000000-0000-0000-0000-000000003100").expect("id");
        let b = AccountId::parse("00000000-0000-0000-0000-000000002400").expect("id");
        assert_eq!(get(Some(dir.path()), "co-1", Slot::In), None);
        set(Some(dir.path()), "co-1", Slot::In, a);
        set(Some(dir.path()), "co-1", Slot::PayBack, b);
        assert_eq!(get(Some(dir.path()), "co-1", Slot::In), Some(a));
        assert_eq!(get(Some(dir.path()), "co-1", Slot::PayBack), Some(b));
        assert_eq!(get(Some(dir.path()), "co-2", Slot::In), None);
        let text = std::fs::read_to_string(dir.path().join(FILE)).expect("file");
        assert!(!text.contains("co-1"), "the company key is hashed");
        assert_eq!(get(None, "co-1", Slot::In), None);
    }
}
