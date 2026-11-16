//! Audit-trail field rules shared by the Audit Trail report (cl-api), the
//! Audit Trail viewer (cl-model) and its exports (P29 WP6).
//!
//! Taxpayer ids and secrets (Q-P28p, non-negotiable 11) never show, at any
//! depth of a before/after image; bookkeeping fields are left out.

/// Bookkeeping fields the trail leaves out, and taxpayer ids and secrets.
pub const QUIET: [&str; 14] = [
    "taxId",
    "tax_id",
    "tin",
    "password",
    "passwordHash",
    "password_hash",
    "id",
    "tenantId",
    "tenant_id",
    "createdAt",
    "updatedAt",
    "created_at",
    "updated_at",
    "companyId",
];

/// Name tokens that mark a taxpayer id or a secret (S2): a field whose
/// name has one of them as a whole word (`ein`, `companyEin`, `smtp_pass`),
/// or runs them together (`apiKey`, `taxId`), never shows. Whole words are
/// compared, so `settings` or `reportKey` still show.
const SECRET_WORDS: [&str; 12] = [
    "ein", "tin", "ssn", "taxid", "pass", "password", "secret", "token", "apikey", "hash", "pin",
    "iban",
];

/// The field's name split into lower-case words (`smtpPassHash` → smtp,
/// pass, hash; `tax_id` → tax, id).
fn words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch == '.' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        cur.extend(ch.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// True for a field that must never show in the trail (a secret or a
/// taxpayer id).
#[must_use]
pub fn secret_field(name: &str) -> bool {
    let w = words(name);
    let joined: String = w.concat();
    w.iter().any(|x| SECRET_WORDS.contains(&x.as_str()))
        || SECRET_WORDS.contains(&joined.as_str())
        || ["taxid", "apikey", "privatekey"]
            .iter()
            .any(|k| joined.contains(k))
}

/// True for a field the trail leaves out: a secret, a taxpayer id, or a
/// bookkeeping field (ids, stamps).
#[must_use]
pub fn hidden_field(name: &str) -> bool {
    QUIET.contains(&name) || secret_field(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_and_tax_ids_are_hidden_but_look_alikes_show() {
        for k in [
            "ein",
            "companyEin",
            "smtp_pass",
            "apiKey",
            "taxId",
            "ssn",
            "passwordHash",
            "privateKeyPem",
            "id",
            "updatedAt",
        ] {
            assert!(hidden_field(k), "{k}");
        }
        for k in [
            "settings",
            "reportKey",
            "memo",
            "total",
            "pinned_name",
            "lines",
        ] {
            assert!(!secret_field(k), "{k}");
        }
        assert!(!hidden_field("memo"));
        assert!(!hidden_field("reportKey"));
    }
}
