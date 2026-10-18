//! Payment terms and payment methods (P11 step 6). The engine keeps both
//! as fixed lists today (`paymentTerms` in its company schema,
//! `PAYMENT_METHODS` in its shared constants); editable lists with discount
//! terms wait for U-43, so the desktop shows them read-only and the forms
//! offer exactly these values.

use crate::text::TextKey;

/// One payment term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Term {
    /// The engine's value.
    pub wire: &'static str,
    /// Owner-facing name.
    pub label: TextKey,
    /// Days until due, when fixed.
    pub due_days: Option<u16>,
}

/// The engine's payment terms, in its order.
pub const TERMS: [Term; 6] = [
    Term {
        wire: "due_on_receipt",
        label: TextKey("terms.due_on_receipt"),
        due_days: Some(0),
    },
    Term {
        wire: "net_15",
        label: TextKey("terms.net_15"),
        due_days: Some(15),
    },
    Term {
        wire: "net_30",
        label: TextKey("terms.net_30"),
        due_days: Some(30),
    },
    Term {
        wire: "net_60",
        label: TextKey("terms.net_60"),
        due_days: Some(60),
    },
    Term {
        wire: "net_90",
        label: TextKey("terms.net_90"),
        due_days: Some(90),
    },
    Term {
        wire: "custom",
        label: TextKey("terms.custom"),
        due_days: None,
    },
];

/// One payment method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaymentMethod {
    /// The engine's value.
    pub wire: &'static str,
    /// Owner-facing name.
    pub label: TextKey,
}

/// The engine's payment methods, in its order.
pub const PAYMENT_METHODS: [PaymentMethod; 5] = [
    PaymentMethod {
        wire: "check",
        label: TextKey("methods.check"),
    },
    PaymentMethod {
        wire: "ach",
        label: TextKey("methods.ach"),
    },
    PaymentMethod {
        wire: "credit_card",
        label: TextKey("methods.credit_card"),
    },
    PaymentMethod {
        wire: "cash",
        label: TextKey("methods.cash"),
    },
    PaymentMethod {
        wire: "other",
        label: TextKey("methods.other"),
    },
];

/// The term for an engine value.
#[must_use]
pub fn term(wire: &str) -> Option<&'static Term> {
    TERMS.iter().find(|t| t.wire == wire)
}

/// The payment method for an engine value.
#[must_use]
pub fn payment_method(wire: &str) -> Option<&'static PaymentMethod> {
    PAYMENT_METHODS.iter().find(|m| m.wire == wire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_lists_match_the_engine() {
        let wires: Vec<_> = TERMS.iter().map(|t| t.wire).collect();
        assert_eq!(
            wires,
            [
                "due_on_receipt",
                "net_15",
                "net_30",
                "net_60",
                "net_90",
                "custom"
            ]
        );
        assert_eq!(term("net_30").and_then(|t| t.due_days), Some(30));
        assert_eq!(term("custom").and_then(|t| t.due_days), None);
        assert_eq!(term("net_45"), None);
        assert_eq!(
            payment_method("credit_card").map(|m| m.label.as_str()),
            Some("methods.credit_card")
        );
        assert_eq!(payment_method("barter"), None);
    }
}
