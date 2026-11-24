//! Which tools a connection may call, from the company's MCP switches (plan D1).
//!
//! One function, [`decide`], answers both `tools/list` (what is shown) and
//! `tools/call` (what is sent on), so a tool is never listed and then
//! refused, or refused in the list and then accepted. The inputs are the
//! company's switches (Q-327, Q-328), the connection's state, its live role
//! (U-297: the engine resolves it on every request, so the host reads it
//! again too) and whether the engine can reinstate a voided transaction
//! (`txn-reinstate-v1`, B5). The engine checks every one of these again
//! (U-299, U-300); this only keeps the bridge from offering what would be
//! refused.

use cl_model::commands::CommandId;
use cl_model::permissions::{Action as RightAction, Need, Rights, command_door, need};

use crate::tools::{self, Action, Class, Kind, TOOLS, Tool};

/// The company's MCP switches (Q-327, Q-329).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    /// MCP on for this company.
    pub on: bool,
    /// Each class on, by [`Class`] order (Read, Create, Edit, Delete/Void,
    /// Send & print).
    pub classes: [bool; 5],
    /// Each class's "Require review", in the same order (Read ignores it).
    pub review: [bool; 5],
}

impl Default for Switches {
    /// Off; when first turned on: Read on, every write off, review on for
    /// every write (Q-328; Send & print off, Q-329).
    fn default() -> Switches {
        Switches {
            on: false,
            classes: [true, false, false, false, false],
            review: [false, true, true, true, true],
        }
    }
}

const fn index(c: Class) -> usize {
    match c {
        Class::Read => 0,
        Class::Create => 1,
        Class::Edit => 2,
        Class::DeleteVoid => 3,
        Class::SendPrint => 4,
    }
}

impl Switches {
    /// Is `class` switched on.
    #[must_use]
    pub const fn class_on(&self, class: Class) -> bool {
        self.classes[index(class)]
    }

    /// Does a change of `class` wait for a person (never for a read).
    #[must_use]
    pub const fn reviewed(&self, class: Class) -> bool {
        !matches!(class, Class::Read) && self.review[index(class)]
    }

    /// Set one class on or off.
    pub fn set(&mut self, class: Class, on: bool) {
        self.classes[index(class)] = on;
    }

    /// Set one class's review.
    pub fn set_review(&mut self, class: Class, on: bool) {
        self.review[index(class)] = on;
    }
}

/// Where a connection stands (U-297, U-300).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// In use.
    Active,
    /// Revoked by an Admin: nothing is answered but the refusal.
    Revoked,
    /// After a restore, the Admin must "Confirm again" (Q-A11k).
    Unconfirmed,
}

/// Everything [`decide`] reads, for one connection, now.
#[derive(Clone, Debug)]
pub struct Policy {
    /// The company's switches.
    pub switches: Switches,
    /// The connection's state.
    pub connection: ConnectionState,
    /// The connection's live rights (its role and template).
    pub rights: Rights,
    /// The engine reports `txn-reinstate-v1` (U-301): Delete/Void can be
    /// undone, so it may be offered (B5).
    pub reinstate: bool,
}

/// What an allowed call does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allowed {
    /// The strongest class it needs.
    pub class: Class,
    /// It waits in MCP Activity until a person accepts it.
    pub review: bool,
}

/// Why a call is refused. [`Refusal::message`] is what the agent is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// MCP is off for the company.
    McpOff,
    /// The connection was revoked.
    Revoked,
    /// The connection must be confirmed again after a restore.
    Unconfirmed,
    /// The class's switch is off.
    SwitchOff(Class),
    /// Delete/Void is not available until the engine can undo it.
    NotYetAvailable,
    /// The connection's role does not allow it.
    NoPermission,
    /// No such tool.
    UnknownTool,
    /// The tool has no such action.
    UnknownAction,
}

impl Refusal {
    /// Plain words for the agent (and, through it, the owner).
    #[must_use]
    pub fn message(self) -> String {
        match self {
            Refusal::McpOff => "AI assistants are turned off for this company. An Admin can turn \
                 them on in ClientLedger: Preferences \u{203a} AI assistants (MCP)."
                .into(),
            Refusal::Revoked => "This connection was revoked in ClientLedger. Ask an Admin to add \
                 a new one and connect again."
                .into(),
            Refusal::Unconfirmed => {
                "This company was restored from a backup. An Admin must choose \
                 \"Confirm again\" for this connection in Preferences \u{203a} AI assistants (MCP)."
                    .into()
            }
            Refusal::SwitchOff(c) => format!(
                "\"{}\" is turned off for AI assistants in this company. An Admin can turn it on \
                 in Preferences \u{203a} AI assistants (MCP).",
                c.label()
            ),
            Refusal::NotYetAvailable => "Deleting and voiding through an AI assistant is not yet \
                 available, because it could not be undone."
                .into(),
            Refusal::NoPermission => "This connection's role does not allow that. An Admin can \
                 change its role in Company \u{203a} Users and Permissions."
                .into(),
            Refusal::UnknownTool => {
                "There is no such tool. List the tools again to see what this connection can use."
                    .into()
            }
            Refusal::UnknownAction => {
                "That tool has no such action. List the tools again to see the actions it takes."
                    .into()
            }
        }
    }
}

/// The windows whose rights decide a tool: its own and its read tool's
/// (`x_change` and `x_send` go with `x_read`). Empty only for the host's
/// own tools.
#[must_use]
pub fn form_commands(tool: &Tool) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = tool.commands.to_vec();
    let base = tool
        .name
        .strip_suffix("_change")
        .or_else(|| tool.name.strip_suffix("_send"));
    if let Some(read) = base.and_then(|b| tools::find(&format!("{b}_read"))) {
        out.extend(read.commands.iter().filter(|c| !tool.commands.contains(c)));
    }
    out
}

fn rights_allow(rights: &Rights, tool: &Tool) -> bool {
    if tools::HOST_TOOLS.contains(&tool.name) {
        return true;
    }
    let action = if tool.kind == Kind::Read {
        RightAction::Read
    } else {
        RightAction::Write
    };
    form_commands(tool).into_iter().any(|c| {
        let id = CommandId::new(c);
        // A route the engine refuses while an account is hidden (P29 B2).
        if !command_door(id).is_none_or(|d| rights.door_open(d)) {
            return false;
        }
        // A form window needs write to open, but reading its records needs
        // only read; a change needs write (a list opened with read only
        // keeps its New button off, P29).
        match need(id) {
            Some(Need::Read(r) | Need::Write(r)) => rights.can(r, action),
            Some(Need::Report(s)) => action == RightAction::Read && rights.allows_report(s),
            Some(Need::AdminOnly) => rights.is_admin(),
            Some(Need::Always) => true,
            None => false,
        }
    })
}
/// May this connection call `action` of `tool` now.
///
/// # Errors
/// The first [`Refusal`] that applies, in this order: connection state,
/// MCP off, Read off (everything off), the tool's classes, Delete/Void not
/// available, the role.
pub fn decide_action(p: &Policy, tool: &Tool, action: &Action) -> Result<Allowed, Refusal> {
    match p.connection {
        ConnectionState::Revoked => return Err(Refusal::Revoked),
        ConnectionState::Unconfirmed => return Err(Refusal::Unconfirmed),
        ConnectionState::Active => {}
    }
    if !p.switches.on {
        return Err(Refusal::McpOff);
    }
    // Read off: nothing at all (a write is never blind).
    if !p.switches.class_on(Class::Read) {
        return Err(Refusal::SwitchOff(Class::Read));
    }
    for &c in action.classes {
        if !p.switches.class_on(c) {
            return Err(Refusal::SwitchOff(c));
        }
        if c == Class::DeleteVoid && !p.reinstate {
            return Err(Refusal::NotYetAvailable);
        }
    }
    if !rights_allow(&p.rights, tool) {
        return Err(Refusal::NoPermission);
    }
    let review = action.classes.iter().any(|&c| p.switches.reviewed(c));
    Ok(Allowed {
        class: action.class(),
        review,
    })
}

/// [`decide_action`] by name.
///
/// # Errors
/// `UnknownTool`, `UnknownAction`, or the action's refusal.
pub fn decide(
    p: &Policy,
    tool: &str,
    action: &str,
) -> Result<(&'static Tool, &'static Action, Allowed), Refusal> {
    let t = tools::find(tool).ok_or(Refusal::UnknownTool)?;
    let a = t.action(action).ok_or(Refusal::UnknownAction)?;
    decide_action(p, t, a).map(|ok| (t, a, ok))
}

/// The tools shown to this connection, each with the actions it may call.
/// A tool none of whose actions is allowed is not shown.
#[must_use]
pub fn visible(p: &Policy) -> Vec<(&'static Tool, Vec<(&'static Action, Allowed)>)> {
    TOOLS
        .iter()
        .filter_map(|t| {
            let actions: Vec<_> = t
                .actions
                .iter()
                .filter_map(|a| decide_action(p, t, a).ok().map(|ok| (a, ok)))
                .collect();
            (!actions.is_empty()).then_some((t, actions))
        })
        .collect()
}

/// A short, stable fingerprint of what [`visible`] shows: the relay polls
/// it and tells the agent `tools/list_changed` when it moves (N5).
#[must_use]
pub fn fingerprint(p: &Policy) -> String {
    // FNV-1a over the shown names: no dependency, stable across runs.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (t, actions) in visible(p) {
        for (a, ok) in actions {
            for b in t
                .name
                .bytes()
                .chain(*b".")
                .chain(a.name.bytes())
                .chain([if ok.review { b'r' } else { b'n' }, b';'])
            {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    format!("{h:016x}")
}

/// What the connection can and cannot do, for `initialize`'s
/// `instructions` (and `mcp_status`).
#[must_use]
pub fn instructions(p: &Policy) -> String {
    let mut s = String::from(
        "ClientLedger bookkeeping for the company open in ClientLedger on this computer. ",
    );
    if let Err(r) = decide_action(p, &STATUS_TOOL, &STATUS_ACTION) {
        s.push_str(&r.message());
        s.push(' ');
        s.push_str(NEVER);
        return s;
    }
    let mut can = Vec::new();
    let mut cannot = Vec::new();
    for c in Class::ALL {
        let state = if !p.switches.class_on(c) {
            Err("turned off")
        } else if c == Class::DeleteVoid && !p.reinstate {
            Err("not yet available")
        } else {
            Ok(())
        };
        match state {
            Ok(()) if p.switches.reviewed(c) => can.push(format!(
                "{} (each change waits for a person to accept it in MCP Activity)",
                c.label()
            )),
            Ok(()) => can.push(c.label().to_string()),
            Err(why) => cannot.push(format!("{} ({why})", c.label())),
        }
    }
    s.push_str("This connection can: ");
    s.push_str(&can.join("; "));
    s.push_str(". ");
    if !cannot.is_empty() {
        s.push_str("It cannot: ");
        s.push_str(&cannot.join("; "));
        s.push_str(". ");
    }
    s.push_str(
        "Its role limits it further. Send every amount as text, like \"125.00\", never as a \
         number. A change that waits returns its action id; follow it with mcp_status \
         (action get_action). Emails cannot be unsent. ",
    );
    s.push_str(NEVER);
    s
}

/// What no connection can ever do (Q-332).
pub const NEVER: &str = "No AI assistant can ever change users, roles or AI assistant \
     settings; the closing date, its password or anything in a closed period; or run a backup, \
     restore or company delete.";

const STATUS_ACTION: Action = Action {
    name: "get",
    classes: &[Class::Read],
    steps: &[],
    record_edit: false,
    what: "",
};
const STATUS_TOOL: Tool = Tool {
    name: "mcp_status",
    commands: &[],
    verbs: &[],
    kind: Kind::Read,
    description: "",
    actions: &[STATUS_ACTION],
};

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> Policy {
        Policy {
            switches: Switches {
                on: true,
                ..Switches::default()
            },
            connection: ConnectionState::Active,
            rights: Rights::all(),
            reinstate: false,
        }
    }

    fn names(p: &Policy) -> Vec<String> {
        visible(p)
            .into_iter()
            .flat_map(|(t, a)| {
                a.into_iter()
                    .map(move |(a, _)| format!("{}.{}", t.name, a.name))
            })
            .collect()
    }

    #[test]
    fn starting_settings_offer_reads_only() {
        let p = on();
        let shown = visible(&p);
        assert_ne!(shown.len(), 0);
        for (t, actions) in &shown {
            assert_eq!(t.kind, Kind::Read, "{}", t.name);
            for (_, ok) in actions {
                assert_eq!(ok.class, Class::Read);
                assert!(!ok.review, "reads never wait");
            }
        }
        assert_eq!(
            decide(&p, "bill_change", "create").map(|x| x.2),
            Err(Refusal::SwitchOff(Class::Create))
        );
    }

    #[test]
    fn off_revoked_and_unconfirmed_show_nothing() {
        let mut p = on();
        p.switches.on = false;
        assert_eq!(visible(&p).len(), 0);
        assert_eq!(
            decide(&p, "bill_read", "list").map(|x| x.2),
            Err(Refusal::McpOff)
        );
        let mut p = on();
        p.connection = ConnectionState::Revoked;
        assert_eq!(visible(&p).len(), 0);
        assert!(instructions(&p).contains("revoked"));
        let mut p = on();
        p.connection = ConnectionState::Unconfirmed;
        assert_eq!(
            decide(&p, "bill_read", "list").map(|x| x.2),
            Err(Refusal::Unconfirmed)
        );
        let mut p = on();
        p.switches.set(Class::Read, false);
        p.switches.set(Class::Create, true);
        assert_eq!(visible(&p).len(), 0, "Read off: everything off");
    }

    #[test]
    fn create_on_with_review_waits() {
        let mut p = on();
        p.switches.set(Class::Create, true);
        let (_, _, ok) = decide(&p, "bill_change", "create").expect("allowed");
        assert_eq!(
            ok,
            Allowed {
                class: Class::Create,
                review: true
            }
        );
        p.switches.set_review(Class::Create, false);
        assert!(
            !decide(&p, "bill_change", "create")
                .expect("allowed")
                .2
                .review
        );
        assert_eq!(
            decide(&p, "bill_change", "edit").map(|x| x.2),
            Err(Refusal::SwitchOff(Class::Edit))
        );
        assert!(names(&p).contains(&"bill_change.create".to_string()));
        assert!(!names(&p).contains(&"bill_change.edit".to_string()));
    }

    #[test]
    fn delete_void_waits_for_reinstate() {
        let mut p = on();
        p.switches.set(Class::DeleteVoid, true);
        assert_eq!(
            decide(&p, "bill_change", "void").map(|x| x.2),
            Err(Refusal::NotYetAvailable)
        );
        assert!(
            !names(&p)
                .iter()
                .any(|n| matches!(n.rsplit('.').next(), Some("void" | "inactivate")))
        );
        assert!(instructions(&p).contains("Delete/Void (not yet available)"));
        p.reinstate = true;
        assert!(decide(&p, "bill_change", "void").is_ok());
    }

    #[test]
    fn a_reconciliation_with_charges_needs_create_too() {
        let mut p = on();
        p.switches.set(Class::Edit, true);
        assert!(decide(&p, "reconcile_change", "reconcile").is_ok());
        assert_eq!(
            decide(&p, "reconcile_change", "reconcile_with_charges").map(|x| x.2),
            Err(Refusal::SwitchOff(Class::Create))
        );
        p.switches.set(Class::Create, true);
        p.switches.set_review(Class::Edit, false);
        // Create still reviews: the whole action waits.
        assert!(
            decide(&p, "reconcile_change", "reconcile_with_charges")
                .expect("ok")
                .2
                .review
        );
    }

    #[test]
    fn send_is_its_own_switch() {
        let mut p = on();
        for c in [Class::Create, Class::Edit] {
            p.switches.set(c, true);
        }
        assert_eq!(
            decide(&p, "invoice_send", "email").map(|x| x.2),
            Err(Refusal::SwitchOff(Class::SendPrint))
        );
        p.switches.set(Class::SendPrint, true);
        assert!(decide(&p, "invoice_send", "email").is_ok());
    }

    #[test]
    fn the_role_limits_the_tools() {
        let mut p = on();
        p.switches.set(Class::Create, true);
        p.rights = Rights::view_only();
        assert!(decide(&p, "bill_read", "list").is_ok());
        assert_eq!(
            decide(&p, "bill_change", "create").map(|x| x.2),
            Err(Refusal::NoPermission)
        );
        p.rights = Rights::none();
        assert_eq!(
            decide(&p, "bill_read", "list").map(|x| x.2),
            Err(Refusal::NoPermission)
        );
        // The status tool is always there while the connection is usable.
        assert!(decide(&p, "mcp_status", "get").is_ok());
    }

    #[test]
    fn unknown_names_are_refused() {
        let p = on();
        assert_eq!(
            decide(&p, "closing_date", "set").map(|x| x.2),
            Err(Refusal::UnknownTool)
        );
        assert_eq!(
            decide(&p, "bill_read", "delete").map(|x| x.2),
            Err(Refusal::UnknownAction)
        );
    }

    #[test]
    fn every_tool_has_a_form_for_its_rights() {
        for t in TOOLS {
            if tools::HOST_TOOLS.contains(&t.name) {
                continue;
            }
            assert!(!form_commands(t).is_empty(), "{}", t.name);
        }
    }

    #[test]
    fn the_fingerprint_moves_with_what_is_shown() {
        let mut p = on();
        let a = fingerprint(&p);
        assert_eq!(a, fingerprint(&p));
        p.switches.set(Class::Create, true);
        let b = fingerprint(&p);
        assert_ne!(a, b);
        p.switches.set_review(Class::Create, false);
        assert_ne!(b, fingerprint(&p), "review changes what a call does");
    }

    #[test]
    fn instructions_say_what_it_can_and_cannot_do() {
        let mut p = on();
        p.switches.set(Class::Create, true);
        let s = instructions(&p);
        assert!(s.contains("can: Read; Create (each change waits"), "{s}");
        assert!(s.contains("It cannot: Edit (turned off)"), "{s}");
        assert!(s.contains("never as a number"), "{s}");
        assert!(s.contains(NEVER));
        let mut off = on();
        off.switches.on = false;
        assert!(instructions(&off).contains("turned off for this company"));
    }
}
