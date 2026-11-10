//! Converts the toolkit's accessibility tree into AccessKit updates, and
//! AccessKit action requests back into toolkit events.

use accesskit::{Action, ActionData, ActionRequest, Live, NodeId, Toggled, TreeId, TreeInfo, TreeUpdate};

use crate::a11y::{A11yAction, Node, Role, Tree};
use crate::input::Event;
use crate::ui::Id;

fn role(r: Role) -> accesskit::Role {
    use accesskit::Role as K;
    match r {
        Role::Window => K::Window,
        Role::Group => K::Group,
        Role::Region => K::Region,
        Role::Button => K::Button,
        Role::CheckBox => K::CheckBox,
        Role::TextInput => K::TextInput,
        Role::PasswordInput => K::PasswordInput,
        Role::List => K::List,
        Role::ListItem => K::ListItem,
        Role::TabList => K::TabList,
        Role::Tab => K::Tab,
        Role::TabPanel => K::TabPanel,
        Role::Menu => K::Menu,
        Role::MenuItem => K::MenuItem,
        Role::ComboBox => K::ComboBox,
        Role::Label => K::Label,
        Role::Terminal => K::Terminal,
        Role::Dialog => K::Dialog,
        Role::Status => K::Status,
        Role::Splitter => K::Splitter,
    }
}

fn node(n: &Node, root_title: &str) -> accesskit::Node {
    let mut k = accesskit::Node::new(role(n.role));
    let name = if n.role == Role::Window && n.name.is_empty() { root_title } else { &n.name };
    if !name.is_empty() {
        k.set_label(name);
    }
    if let Some(v) = &n.value {
        k.set_value(v.as_str());
    }
    let b = n.bounds;
    k.set_bounds(accesskit::Rect { x0: b.x as f64, y0: b.y as f64, x1: b.right() as f64, y1: b.bottom() as f64 });
    if let Some(s) = n.selected {
        k.set_selected(s);
    }
    if let Some(c) = n.checked {
        k.set_toggled(if c { Toggled::True } else { Toggled::False });
    }
    if let Some(e) = n.expanded {
        k.set_expanded(e);
    }
    if n.disabled {
        k.set_disabled();
    }
    if n.live {
        k.set_live(Live::Polite);
    }
    if !n.children.is_empty() {
        k.set_children(n.children.iter().map(|c| NodeId(c.0)).collect::<Vec<_>>());
    }
    match n.role {
        Role::Button | Role::CheckBox | Role::ComboBox => {
            k.add_action(Action::Click);
            k.add_action(Action::Focus);
        }
        // Rows, tabs and menu items are reached through their container.
        Role::ListItem | Role::Tab | Role::MenuItem => k.add_action(Action::Click),
        Role::TextInput => {
            k.add_action(Action::Focus);
            k.add_action(Action::SetValue);
        }
        Role::PasswordInput | Role::List | Role::Terminal => k.add_action(Action::Focus),
        Role::Splitter => {
            k.add_action(Action::Focus);
            k.add_action(Action::Increment);
            k.add_action(Action::Decrement);
        }
        _ => {}
    }
    k
}

/// A full-tree update. Bounds are physical pixels relative to the window.
pub fn to_update(tree: &Tree, window_title: &str) -> TreeUpdate {
    let root = tree.nodes.first().map_or(NodeId(crate::a11y::ROOT.0), |n| NodeId(n.id.0));
    let mut info = TreeInfo::new(root);
    info.toolkit_name = Some("skiff-ui".into());
    TreeUpdate {
        nodes: tree.nodes.iter().map(|n| (NodeId(n.id.0), node(n, window_title))).collect(),
        tree: Some(info),
        tree_id: TreeId::ROOT,
        focus: tree.focus.map_or(root, |f| NodeId(f.0)),
    }
}

/// The toolkit event for a screen reader's request, if the toolkit supports it.
pub fn to_event(req: ActionRequest) -> Option<Event> {
    let action = match (req.action, req.data) {
        (Action::Click, _) => A11yAction::Click,
        (Action::Focus, _) => A11yAction::Focus,
        (Action::SetValue, Some(ActionData::Value(v))) => A11yAction::SetValue(v.into()),
        (Action::Increment, _) => A11yAction::Increment,
        (Action::Decrement, _) => A11yAction::Decrement,
        (Action::ScrollIntoView, _) => A11yAction::ScrollIntoView,
        _ => return None,
    };
    Some(Event::A11y { target: Id(req.target_node.0), action })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;

    #[test]
    fn to_update_maps_roles_and_focus() {
        let mut root = Node::new(crate::a11y::ROOT, Role::Window, "");
        let mut list = Node::new(Id(10), Role::List, "Hosts");
        let mut a = Node::new(Id(11), Role::ListItem, "alpha");
        a.selected = Some(false);
        let mut b = Node::new(Id(12), Role::ListItem, "beta");
        b.selected = Some(true);
        b.bounds = Rect::new(0, 20, 100, 20);
        list.children = vec![a.id, b.id];
        root.children = vec![list.id];
        let tree = Tree { nodes: vec![root, list, a, b], focus: Some(Id(12)) };
        let u = to_update(&tree, "skiff");
        assert_eq!(u.tree.as_ref().map(|t| t.root), Some(NodeId(1)));
        assert_eq!(u.focus, NodeId(12));
        let get = |id: u64| &u.nodes.iter().find(|(n, _)| n.0 == id).unwrap().1;
        assert_eq!(get(1).label(), Some("skiff"));
        assert_eq!(get(10).children(), &[NodeId(11), NodeId(12)]);
        assert_eq!((get(12).role(), get(12).label(), get(12).is_selected()), (accesskit::Role::ListItem, Some("beta"), Some(true)));
        assert_eq!(get(12).bounds().map(|r| r.y1), Some(40.0));
    }

    #[test]
    fn action_requests_become_events() {
        let req = |action, data| ActionRequest { action, target_tree: TreeId::ROOT, target_node: NodeId(7), data };
        assert_eq!(to_event(req(Action::Click, None)), Some(Event::A11y { target: Id(7), action: A11yAction::Click }));
        let set = to_event(req(Action::SetValue, Some(ActionData::Value("x".into()))));
        assert_eq!(set, Some(Event::A11y { target: Id(7), action: A11yAction::SetValue("x".into()) }));
        assert_eq!(to_event(req(Action::ShowTooltip, None)), None);
    }
}
