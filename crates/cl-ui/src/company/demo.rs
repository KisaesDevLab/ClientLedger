//! Synthetic data for `--demo` (the mockups' companies). Nothing here is
//! real; the figures come from `docs/design/README.md`.

use cl_core::ids::TenantId;
use cl_core::{CalendarDate, Timestamp};
use cl_settings::{ACCENT_PALETTE, Accent};

/// One demo company card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoCompany {
    /// Fake tenant id.
    pub tenant_id: TenantId,
    /// Name.
    pub name: String,
    /// Stripe.
    pub accent: Accent,
    /// Meta line as the mockup shows it.
    pub meta: String,
    /// The practice company.
    pub is_sample: bool,
    /// Closing date shown in the title bar when opened.
    pub closed_through: Option<CalendarDate>,
}

/// Demo-mode state the screens share.
#[derive(Clone, Debug)]
pub struct DemoState {
    /// The four companies of the Open Company mockup.
    pub companies: Vec<DemoCompany>,
    /// Which one is open.
    pub open: Option<TenantId>,
}

fn id(n: u8) -> TenantId {
    TenantId::parse(&format!("d0000000-0000-4000-8000-0000000000{n:02x}")).expect("demo id")
}

impl DemoState {
    /// The mockup list.
    #[must_use]
    pub fn new() -> DemoState {
        DemoState {
            companies: vec![
                DemoCompany {
                    tenant_id: id(1),
                    name: "Ozark Ridge Landscaping LLC".into(),
                    accent: ACCENT_PALETTE[0],
                    meta: "Last opened today 8:10 AM \u{b7} Backed up today 8:12 AM \u{b7} 1,512 transactions"
                        .into(),
                    is_sample: false,
                    closed_through: CalendarDate::from_ymd(2026, 9, 30).ok(),
                },
                DemoCompany {
                    tenant_id: id(2),
                    name: "Shoal Creek Farm".into(),
                    accent: ACCENT_PALETTE[1],
                    meta: "Last opened 10/28/2026 \u{b7} Converted from QuickBooks Desktop \u{b7} Verified"
                        .into(),
                    is_sample: false,
                    closed_through: None,
                },
                DemoCompany {
                    tenant_id: id(3),
                    name: "Kurt \u{2014} Rental Duplex".into(),
                    accent: ACCENT_PALETTE[2],
                    meta: "Last opened 10/02/2026 \u{b7} Backup reminder: last backup 35 days ago".into(),
                    is_sample: false,
                    closed_through: None,
                },
                DemoCompany {
                    tenant_id: id(4),
                    name: "Practice company \u{b7} Ozark Ridge (sample)".into(),
                    accent: Accent([0xC9, 0xC4, 0xBA]),
                    meta: "Made-up data to try things safely \u{b7} Reset any time".into(),
                    is_sample: true,
                    closed_through: None,
                },
            ],
            open: Some(id(1)),
        }
    }

    /// The story date of the mockups.
    #[must_use]
    pub fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).unwrap_or_else(|_| CalendarDate::min())
    }

    /// A fixed timestamp for demo records.
    #[must_use]
    pub fn now() -> Timestamp {
        Timestamp::parse_wire("2026-11-04T08:10:00.000Z").unwrap_or_else(|_| {
            Timestamp::from_unix_millis(0).unwrap_or_else(|_| unreachable!("epoch"))
        })
    }

    /// A fresh fake tenant id for a company made in the demo.
    #[must_use]
    pub fn fresh_id(&self) -> TenantId {
        let n = u8::try_from(self.companies.len() + 1).unwrap_or(u8::MAX);
        id(n)
    }
}

impl Default for DemoState {
    fn default() -> Self {
        DemoState::new()
    }
}

/// The demo recovery key shown by the recovery-key screen (synthetic;
/// redacted by `inspect` like any key).
pub const DEMO_RECOVERY_KEY: &str = "K7F2-9QMD-3XTB-8RVC-PL6N-WY4H";

/// Demo chart-of-accounts names for the opening-balances pickers.
pub const DEMO_ACCOUNTS: [(&str, &str, &str); 8] = [
    ("1010", "Operating Checking", "asset"),
    ("1020", "Savings", "asset"),
    ("2050", "Business Credit Card", "liability"),
    ("2200", "Vehicle and Equipment Loans", "liability"),
    ("1500", "Equipment", "asset"),
    ("1510", "Vehicles", "asset"),
    ("3100", "Owner Contributions", "equity"),
    ("2300", "Line of Credit", "liability"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_list_is_the_mockup() {
        let d = DemoState::new();
        assert_eq!(d.companies.len(), 4);
        assert_eq!(d.companies[0].name, "Ozark Ridge Landscaping LLC");
        assert_eq!(d.open, Some(d.companies[0].tenant_id));
        assert!(d.companies[3].is_sample);
        assert!(d.companies[0].closed_through.is_some());
        let fresh = d.fresh_id();
        assert!(d.companies.iter().all(|c| c.tenant_id != fresh));
        assert_eq!(DemoState::today().to_wire(), "2026-11-04");
        assert_eq!(DemoState::now().to_wire(), "2026-11-04T08:10:00.000Z");
        assert_eq!(DemoState::default().companies.len(), 4);
        assert_eq!(DEMO_ACCOUNTS.len(), 8);
        assert!(DEMO_RECOVERY_KEY.contains('-'));
    }
}
