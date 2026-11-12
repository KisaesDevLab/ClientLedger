//! The definition tree in pre-order (A07 plan §3): one walk that every rule
//! uses, so "earlier", node counts and paths mean the same thing
//! everywhere. A08's editor builds on it.

use super::schema::Node;

/// One node met by [`walk`].
#[derive(Clone, Debug)]
pub struct Visit<'a> {
    /// The node.
    pub node: &'a Node,
    /// Its place in pre-order (0 = first).
    pub order: usize,
    /// Depth (1 = top level, as the engine counts).
    pub depth: usize,
    /// Its path from `definition` (`definition.nodes.0.children.2`), as the
    /// engine reports it in `DEFINITION_INVALID {path}`.
    pub path: String,
    /// The ids of the groups it lies under, outermost first.
    pub ancestors: Vec<&'a str>,
}

/// Every node of `nodes` at any depth, in pre-order.
#[must_use]
pub fn walk(nodes: &[Node]) -> Vec<Visit<'_>> {
    let mut out = Vec::new();
    // An explicit stack: no recursion on owner data.
    let mut stack: Vec<(&Node, usize, String, Vec<&str>)> = nodes
        .iter()
        .enumerate()
        .rev()
        .map(|(i, n)| (n, 1, format!("definition.nodes.{i}"), Vec::new()))
        .collect();
    while let Some((node, depth, path, ancestors)) = stack.pop() {
        let mut inner = ancestors.clone();
        inner.push(node.id());
        for (i, c) in node.children().iter().enumerate().rev() {
            stack.push((c, depth + 1, format!("{path}.children.{i}"), inner.clone()));
        }
        let order = out.len();
        out.push(Visit {
            node,
            order,
            depth,
            path,
            ancestors,
        });
    }
    out
}

/// How many nodes the tree has, at every depth (one rule on both sides).
#[must_use]
pub fn count(nodes: &[Node]) -> usize {
    walk(nodes).len()
}

/// The node with `id`, if any.
#[must_use]
pub fn find<'a>(nodes: &'a [Node], id: &str) -> Option<&'a Node> {
    walk(nodes)
        .into_iter()
        .find(|v| v.node.id() == id)
        .map(|v| v.node)
}

// ---- A08: the editor's pure operations on the tree (plan §3.1) ----

/// Why an outline operation can't be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineError {
    /// No node has that id.
    NotFound,
    /// The node would sit deeper than 8 levels (`TOO_DEEP`).
    TooDeep,
    /// Indent needs a group directly above (Q-A08s).
    NoGroupAbove,
}

/// The index path of `id` (`[0, 2]` is `nodes[0].children[2]`), read from
/// the walk's own path so both always agree.
#[must_use]
pub fn locate(nodes: &[Node], id: &str) -> Option<Vec<usize>> {
    let v = walk(nodes).into_iter().find(|v| v.node.id() == id)?;
    Some(
        v.path
            .split('.')
            .filter_map(|p| p.parse::<usize>().ok())
            .collect(),
    )
}

/// The sibling list at `parent` (an index path to a group, or `[]` for the
/// top level).
fn siblings_mut<'a>(nodes: &'a mut Vec<Node>, parent: &[usize]) -> Option<&'a mut Vec<Node>> {
    let mut list = nodes;
    for &i in parent {
        match list.get_mut(i)? {
            Node::Group(g) => list = &mut g.children,
            _ => return None,
        }
    }
    Some(list)
}

/// The levels a node's subtree spans (1 for a leaf).
fn height(node: &Node) -> usize {
    walk(std::slice::from_ref(node))
        .iter()
        .map(|v| v.depth)
        .max()
        .unwrap_or(1)
}

/// Inserts `node` after `selected` at the same level, or as the last row
/// of `selected` when it is a group shown open (`into_group`); at the end
/// of the top level when nothing is selected.
///
/// # Errors
/// [`OutlineError::NotFound`] for an unknown selection;
/// [`OutlineError::TooDeep`] past 8 levels.
pub fn insert_after(
    nodes: &mut Vec<Node>,
    selected: Option<&str>,
    into_group: bool,
    node: Node,
) -> Result<(), OutlineError> {
    let Some(sel) = selected else {
        nodes.push(node);
        return Ok(());
    };
    let path = locate(nodes, sel).ok_or(OutlineError::NotFound)?;
    let is_group = matches!(find(nodes, sel), Some(Node::Group(_)));
    let (parent, at, depth) = if into_group && is_group {
        (path.clone(), usize::MAX, path.len() + 1)
    } else {
        let (last, parent) = path.split_last().ok_or(OutlineError::NotFound)?;
        (parent.to_vec(), last + 1, path.len())
    };
    if depth + height(&node) - 1 > super::validate::limits::DEPTH {
        return Err(OutlineError::TooDeep);
    }
    let list = siblings_mut(nodes, &parent).ok_or(OutlineError::NotFound)?;
    let at = at.min(list.len());
    list.insert(at, node);
    Ok(())
}

/// Removes `id` and everything under it; returns the ids removed
/// (`None`: no such node).
pub fn remove(nodes: &mut Vec<Node>, id: &str) -> Option<Vec<String>> {
    let path = locate(nodes, id)?;
    let (last, parent) = path.split_last()?;
    let list = siblings_mut(nodes, parent)?;
    let gone = list.remove(*last);
    Some(
        walk(std::slice::from_ref(&gone))
            .iter()
            .map(|v| v.node.id().to_string())
            .collect(),
    )
}

/// Drops every formula term naming one of `ids`; returns how many.
pub fn drop_terms(nodes: &mut [Node], ids: &[String]) -> usize {
    let mut dropped = 0;
    for n in nodes {
        match n {
            Node::Formula(f) => {
                let before = f.terms.len();
                f.terms.retain(|t| !ids.contains(&t.node_id));
                dropped += before - f.terms.len();
            }
            Node::Group(g) => dropped += drop_terms(&mut g.children, ids),
            _ => {}
        }
    }
    dropped
}

/// Swaps `id` with the sibling before it (`up`) or after it. At a group's
/// edge nothing moves (Q-A08s). Returns true when it moved.
pub fn shift(nodes: &mut Vec<Node>, id: &str, up: bool) -> bool {
    let Some(path) = locate(nodes, id) else {
        return false;
    };
    let Some((&last, parent)) = path.split_last() else {
        return false;
    };
    let Some(list) = siblings_mut(nodes, parent) else {
        return false;
    };
    let other = if up {
        last.checked_sub(1)
    } else {
        Some(last + 1).filter(|&o| o < list.len())
    };
    match other {
        Some(o) => {
            list.swap(last, o);
            true
        }
        None => false,
    }
}

/// Makes `id` the last row of the group directly before it.
///
/// # Errors
/// [`OutlineError::NoGroupAbove`] when the row before is not a group (or
/// there is none); [`OutlineError::TooDeep`] past 8 levels.
pub fn indent(nodes: &mut Vec<Node>, id: &str) -> Result<(), OutlineError> {
    let path = locate(nodes, id).ok_or(OutlineError::NotFound)?;
    let (&last, parent) = path.split_last().ok_or(OutlineError::NotFound)?;
    let list = siblings_mut(nodes, parent).ok_or(OutlineError::NotFound)?;
    let above = last.checked_sub(1).ok_or(OutlineError::NoGroupAbove)?;
    if !matches!(list.get(above), Some(Node::Group(_))) {
        return Err(OutlineError::NoGroupAbove);
    }
    if path.len() + height(&list[last]) > super::validate::limits::DEPTH {
        return Err(OutlineError::TooDeep);
    }
    let node = list.remove(last);
    if let Some(Node::Group(g)) = list.get_mut(above) {
        g.children.push(node);
    }
    Ok(())
}

/// Moves `id` out of its group, right after it (Q-A08s). Returns true when
/// it moved (false at the top level).
pub fn outdent(nodes: &mut Vec<Node>, id: &str) -> bool {
    let Some(path) = locate(nodes, id) else {
        return false;
    };
    if path.len() < 2 {
        return false;
    }
    let (&last, parent) = path.split_last().unwrap_or((&0, &[]));
    let Some((&group_at, outer)) = parent.split_last() else {
        return false;
    };
    let Some(list) = siblings_mut(nodes, parent) else {
        return false;
    };
    let node = list.remove(last);
    let Some(outer_list) = siblings_mut(nodes, outer) else {
        return false;
    };
    outer_list.insert(group_at + 1, node);
    true
}

/// Sets a row's caption (sections, groups, formulas) or text (text rows).
/// Returns false for rows with neither (blank rows, page breaks) or an
/// unknown id.
pub fn rename(nodes: &mut Vec<Node>, id: &str, text: &str) -> bool {
    let Some(path) = locate(nodes, id) else {
        return false;
    };
    let Some((&last, parent)) = path.split_last() else {
        return false;
    };
    let Some(list) = siblings_mut(nodes, parent) else {
        return false;
    };
    match &mut list[last] {
        Node::Section(s) => s.caption = text.to_string(),
        Node::Group(g) => g.caption = text.to_string(),
        Node::Formula(f) => f.caption = text.to_string(),
        Node::Text(t) => t.text = text.to_string(),
        Node::Blank(_) | Node::PageBreak(_) => return false,
    }
    true
}

/// Replaces the node with `node`'s id by `node`; a group keeps its
/// children (the properties forms edit a row, never its rows). Returns
/// false for an unknown id.
pub fn replace(nodes: &mut Vec<Node>, node: Node) -> bool {
    let Some(path) = locate(nodes, node.id()) else {
        return false;
    };
    let Some((&last, parent)) = path.split_last() else {
        return false;
    };
    let Some(list) = siblings_mut(nodes, parent) else {
        return false;
    };
    let mut node = node;
    if let (Node::Group(old), Node::Group(new)) = (&mut list[last], &mut node) {
        new.children = std::mem::take(&mut old.children);
    }
    list[last] = node;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report_def::schema::{Formula, Group, Spacer, Term};

    fn blank(id: &str) -> Node {
        Node::Blank(Spacer { id: id.into() })
    }

    fn group(id: &str, children: Vec<Node>) -> Node {
        Node::Group(Group {
            id: id.into(),
            caption: id.into(),
            children,
            show_total: false,
            total_caption: None,
        })
    }

    #[test]
    fn the_walk_is_pre_order_with_paths_and_ancestors() {
        let nodes = vec![
            group("a", vec![blank("a1"), group("a2", vec![blank("a2x")])]),
            blank("b"),
        ];
        let v = walk(&nodes);
        let ids: Vec<&str> = v.iter().map(|x| x.node.id()).collect();
        assert_eq!(ids, ["a", "a1", "a2", "a2x", "b"]);
        assert_eq!(v[3].path, "definition.nodes.0.children.1.children.0");
        assert_eq!(v[3].ancestors, ["a", "a2"]);
        assert_eq!(v[3].depth, 3);
        assert_eq!(v[4].order, 4);
        assert_eq!(count(&nodes), 5);
        assert!(find(&nodes, "a2x").is_some());
        assert!(find(&nodes, "zz").is_none());
    }

    fn ids(nodes: &[Node]) -> Vec<String> {
        walk(nodes)
            .iter()
            .map(|v| format!("{}{}", "  ".repeat(v.depth - 1), v.node.id()))
            .collect()
    }

    fn formula(id: &str, terms: &[&str]) -> Node {
        Node::Formula(Formula {
            id: id.into(),
            caption: id.into(),
            terms: terms
                .iter()
                .map(|t| Term {
                    node_id: (*t).into(),
                    sign: 1,
                })
                .collect(),
        })
    }

    #[test]
    fn insert_goes_after_the_selection_or_into_an_open_group() {
        let mut n = vec![group("g", vec![blank("a")]), blank("b")];
        insert_after(&mut n, Some("a"), false, blank("x")).unwrap();
        insert_after(&mut n, Some("g"), true, blank("y")).unwrap();
        insert_after(&mut n, Some("g"), false, blank("z")).unwrap();
        insert_after(&mut n, None, false, blank("end")).unwrap();
        assert_eq!(ids(&n), ["g", "  a", "  x", "  y", "z", "b", "end"]);
        assert_eq!(
            insert_after(&mut n, Some("nope"), false, blank("q")),
            Err(OutlineError::NotFound)
        );
        // A blank row selected "into" is not a group: after it.
        insert_after(&mut n, Some("b"), true, blank("c")).unwrap();
        assert_eq!(locate(&n, "c"), Some(vec![3]));
    }

    fn chain(depth: usize) -> Node {
        let mut node = blank("leaf");
        for d in (1..depth).rev() {
            node = group(&format!("g{d}"), vec![node]);
        }
        node
    }

    #[test]
    fn nothing_goes_deeper_than_eight() {
        // g1 > ... > g7 > leaf: the leaf is at depth 8.
        let mut n = vec![chain(8)];
        insert_after(&mut n, Some("g7"), true, blank("x")).unwrap();
        assert_eq!(locate(&n, "x").map(|p| p.len()), Some(8));
        // An empty group at depth 8 can't take a row.
        insert_after(&mut n, Some("x"), false, group("full", vec![])).unwrap();
        assert_eq!(
            insert_after(&mut n, Some("full"), true, blank("y")),
            Err(OutlineError::TooDeep)
        );
        assert_eq!(
            insert_after(&mut n, Some("x"), false, group("deep", vec![blank("y")])),
            Err(OutlineError::TooDeep)
        );
        // Indent: a row may go to depth 8, a group with rows not.
        let mut m = vec![chain(7)];
        insert_after(&mut m, Some("leaf"), false, group("h", vec![])).unwrap();
        insert_after(&mut m, Some("h"), false, blank("i")).unwrap();
        indent(&mut m, "i").unwrap();
        assert_eq!(locate(&m, "i").map(|p| p.len()), Some(8));
        insert_after(&mut m, Some("h"), false, group("h2", vec![blank("k")])).unwrap();
        assert_eq!(indent(&mut m, "h2"), Err(OutlineError::TooDeep));
    }

    #[test]
    fn remove_takes_the_subtree_and_terms_naming_it_can_be_dropped() {
        let mut n = vec![
            group("g", vec![blank("a"), blank("b")]),
            formula("f", &["g", "a", "z"]),
        ];
        let gone = remove(&mut n, "g").unwrap();
        assert_eq!(gone, ["g", "a", "b"]);
        assert_eq!(drop_terms(&mut n, &gone), 2);
        let Node::Formula(f) = &n[0] else { panic!() };
        assert_eq!(f.terms.len(), 1);
        assert!(remove(&mut n, "g").is_none());
    }

    #[test]
    fn moves_stop_at_a_group_edge() {
        let mut n = vec![group("g", vec![blank("a"), blank("b")]), blank("c")];
        assert!(!shift(&mut n, "a", true));
        assert!(shift(&mut n, "a", false));
        assert!(!shift(&mut n, "a", false));
        assert_eq!(ids(&n), ["g", "  b", "  a", "c"]);
        assert!(shift(&mut n, "c", true));
        assert_eq!(ids(&n), ["c", "g", "  b", "  a"]);
        assert!(!shift(&mut n, "zz", true));
    }

    #[test]
    fn indent_needs_a_group_above_and_outdent_lands_after_the_group() {
        let mut n = vec![blank("x"), group("g", vec![blank("a")]), blank("b")];
        assert_eq!(indent(&mut n, "x"), Err(OutlineError::NoGroupAbove));
        assert_eq!(indent(&mut n, "g"), Err(OutlineError::NoGroupAbove));
        indent(&mut n, "b").unwrap();
        assert_eq!(ids(&n), ["x", "g", "  a", "  b"]);
        assert!(outdent(&mut n, "a"));
        assert_eq!(ids(&n), ["x", "g", "  b", "a"]);
        assert!(!outdent(&mut n, "x"));
        assert_eq!(indent(&mut n, "zz"), Err(OutlineError::NotFound));
    }

    #[test]
    fn rename_and_replace_keep_a_groups_rows() {
        let mut n = vec![group("g", vec![blank("a")]), blank("b")];
        assert!(rename(&mut n, "g", "Income"));
        assert!(!rename(&mut n, "b", "x"));
        assert!(replace(&mut n, group("g", vec![])));
        let Node::Group(g) = &n[0] else { panic!() };
        assert_eq!((g.caption.as_str(), g.children.len()), ("g", 1));
        assert!(!replace(&mut n, blank("zz")));
    }
}
