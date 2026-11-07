//! Quick Add (Q-PR4d, QA-13): a name typed in a name field that is not on
//! the list. The owner chooses: **Quick Add** (the name alone, of the
//! field's kind), **Set Up** (the full new-name form, prefilled, back to
//! the form when saved) or **Cancel**. Enter is Quick Add and Esc is
//! Cancel. A field that takes more than one kind (a payee) asks which kind,
//! starting from the field's usual one.
//!
//! The prompt's state and decisions only; the window draws it and sends the
//! create.

/// The kind of name a field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameKind {
    /// A client (customer).
    Client,
    /// A vendor.
    Vendor,
}

impl NameKind {
    /// i18n key of the kind's name.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            NameKind::Client => "quick_add.kind.client",
            NameKind::Vendor => "quick_add.kind.vendor",
        }
    }
}

/// What a name field takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accepts {
    /// One kind (a client field, a vendor field).
    One(NameKind),
    /// Clients or vendors (a payee); the prompt starts on this kind.
    Either(NameKind),
}

impl Accepts {
    /// The kind the prompt starts on.
    #[must_use]
    pub fn first(self) -> NameKind {
        match self {
            Accepts::One(k) | Accepts::Either(k) => k,
        }
    }

    /// True when the prompt asks which kind.
    #[must_use]
    pub fn asks_kind(self) -> bool {
        matches!(self, Accepts::Either(_))
    }
}

/// The owner's choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Add the name alone, of this kind.
    QuickAdd(NameKind),
    /// Open the full new-name form of this kind, prefilled.
    SetUp(NameKind),
    /// Leave the name field as it is (the "pick a name" hint stays).
    Cancel,
}

/// A key the prompt answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKey {
    /// Enter: Quick Add.
    Enter,
    /// Esc: Cancel.
    Escape,
}

/// Where the prompt is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Waiting for a choice.
    Asking,
    /// The Quick Add create is on its way.
    Adding,
    /// The create failed (the message shows; the choices stay).
    Failed,
}

/// The Quick Add prompt of one name field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    name: String,
    accepts: Accepts,
    kind: NameKind,
    stage: Stage,
}

/// The longest name the engine takes (display name, 1-255 characters).
pub const NAME_MAX_CHARS: usize = 255;

impl Prompt {
    /// The prompt for `typed` (trimmed); `None` for a blank or too-long
    /// name (the field's own hint covers it).
    #[must_use]
    pub fn new(typed: &str, accepts: Accepts) -> Option<Prompt> {
        let name = typed.trim();
        if name.is_empty() || name.chars().count() > NAME_MAX_CHARS {
            return None;
        }
        Some(Prompt {
            name: name.to_string(),
            accepts,
            kind: accepts.first(),
            stage: Stage::Asking,
        })
    }

    /// The name as it will be added.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the field takes.
    #[must_use]
    pub fn accepts(&self) -> Accepts {
        self.accepts
    }

    /// The kind chosen so far.
    #[must_use]
    pub fn kind(&self) -> NameKind {
        self.kind
    }

    /// Where the prompt is.
    #[must_use]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Picks the kind (only a field that takes both kinds changes it).
    pub fn set_kind(&mut self, kind: NameKind) {
        if self.accepts.asks_kind() && self.stage != Stage::Adding {
            self.kind = kind;
        }
    }

    /// The choice a key makes (none while the create is on its way).
    #[must_use]
    pub fn on_key(&self, key: PromptKey) -> Option<Choice> {
        if self.stage == Stage::Adding {
            return None;
        }
        Some(match key {
            PromptKey::Enter => Choice::QuickAdd(self.kind),
            PromptKey::Escape => Choice::Cancel,
        })
    }

    /// The Quick Add create was sent.
    pub fn adding(&mut self) {
        self.stage = Stage::Adding;
    }

    /// The create failed: the choices come back.
    pub fn failed(&mut self) {
        self.stage = Stage::Failed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_or_too_long_name_offers_nothing() {
        assert_eq!(Prompt::new("  ", Accepts::One(NameKind::Client)), None);
        let long = "x".repeat(NAME_MAX_CHARS + 1);
        assert_eq!(Prompt::new(&long, Accepts::One(NameKind::Client)), None);
        let p = Prompt::new("  Bluff Hardware ", Accepts::One(NameKind::Vendor)).expect("p");
        assert_eq!(p.name(), "Bluff Hardware");
        assert_eq!(p.stage(), Stage::Asking);
    }

    #[test]
    fn enter_quick_adds_and_esc_cancels() {
        let p = Prompt::new("Bluff Hardware", Accepts::One(NameKind::Vendor)).expect("p");
        assert_eq!(
            p.on_key(PromptKey::Enter),
            Some(Choice::QuickAdd(NameKind::Vendor))
        );
        assert_eq!(p.on_key(PromptKey::Escape), Some(Choice::Cancel));
    }

    #[test]
    fn only_a_payee_asks_the_kind() {
        let mut one = Prompt::new("A", Accepts::One(NameKind::Client)).expect("p");
        one.set_kind(NameKind::Vendor);
        assert_eq!(one.kind(), NameKind::Client);
        assert!(!one.accepts().asks_kind());
        let mut either = Prompt::new("A", Accepts::Either(NameKind::Vendor)).expect("p");
        assert_eq!(either.kind(), NameKind::Vendor);
        either.set_kind(NameKind::Client);
        assert_eq!(
            either.on_key(PromptKey::Enter),
            Some(Choice::QuickAdd(NameKind::Client))
        );
    }

    #[test]
    fn keys_wait_while_adding_and_come_back_after_a_failure() {
        let mut p = Prompt::new("A", Accepts::Either(NameKind::Vendor)).expect("p");
        p.adding();
        assert_eq!(p.on_key(PromptKey::Enter), None);
        p.set_kind(NameKind::Client);
        assert_eq!(p.kind(), NameKind::Vendor);
        p.failed();
        assert_eq!(p.stage(), Stage::Failed);
        assert_eq!(p.on_key(PromptKey::Escape), Some(Choice::Cancel));
        assert_eq!(NameKind::Client.key(), "quick_add.kind.client");
    }
}
