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
    /// Depth (0 = top level).
    pub depth: usize,
    /// Its JSON path (`nodes.0.children.2`), as the engine reports it in
    /// `DEFINITION_INVALID {path}`.
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
        .map(|(i, n)| (n, 0, format!("nodes.{i}"), Vec::new()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report_def::schema::{Group, Spacer};

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
        assert_eq!(v[3].path, "nodes.0.children.1.children.0");
        assert_eq!(v[3].ancestors, ["a", "a2"]);
        assert_eq!(v[3].depth, 2);
        assert_eq!(v[4].order, 4);
        assert_eq!(count(&nodes), 5);
        assert!(find(&nodes, "a2x").is_some());
        assert!(find(&nodes, "zz").is_none());
    }
}
