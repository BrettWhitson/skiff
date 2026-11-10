//! The accessibility tree: widgets describe themselves (role, name, state)
//! while they draw, and the shell hands the result to the platform screen
//! reader through AccessKit. Building is skipped entirely unless an
//! assistive technology asked for the tree.

use std::collections::HashMap;

use crate::geom::Rect;
use crate::ui::Id;

/// The window root's id (stable across frames).
pub const ROOT: Id = Id(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Window,
    Group,
    Region,
    Button,
    CheckBox,
    TextInput,
    PasswordInput,
    List,
    ListItem,
    TabList,
    Tab,
    TabPanel,
    Menu,
    MenuItem,
    ComboBox,
    Label,
    Terminal,
    Dialog,
    Status,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: Id,
    pub role: Role,
    pub name: String,
    pub value: Option<String>,
    /// Physical pixels.
    pub bounds: Rect,
    pub focused: bool,
    pub selected: Option<bool>,
    pub checked: Option<bool>,
    pub expanded: Option<bool>,
    pub disabled: bool,
    /// Changes are announced politely (status text, terminal output).
    pub live: bool,
    pub children: Vec<Id>,
}

impl Node {
    pub fn new(id: Id, role: Role, name: &str) -> Node {
        Node {
            id,
            role,
            name: name.to_owned(),
            value: None,
            bounds: Rect::default(),
            focused: false,
            selected: None,
            checked: None,
            expanded: None,
            disabled: false,
            live: false,
            children: vec![],
        }
    }
}

/// One frame's tree. `nodes[0]` is the window root.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub focus: Option<Id>,
}

impl Tree {
    /// The first node named exactly `name`.
    pub fn find(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }

    pub fn by_id(&self, id: Id) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// What a screen reader asks a widget to do.
#[derive(Debug, Clone, PartialEq)]
pub enum A11yAction {
    Click,
    Focus,
    SetValue(String),
    Increment,
    Decrement,
    ScrollIntoView,
}

/// Collects nodes during a frame.
pub(crate) struct Builder {
    nodes: Vec<Node>,
    index: HashMap<Id, usize>,
    stack: Vec<usize>,
    /// A widget that represents focus by a child (a list's selected row).
    pub focus_override: Option<Id>,
}

impl Builder {
    pub fn new(title: &str, bounds: Rect) -> Builder {
        let mut root = Node::new(ROOT, Role::Window, title);
        root.bounds = bounds;
        Builder { nodes: vec![root], index: HashMap::from([(ROOT, 0)]), stack: vec![0], focus_override: None }
    }

    /// Adds a node under the current parent. A second node with the same id
    /// in one frame replaces nothing: the first one wins.
    pub fn add(&mut self, node: Node) -> &mut Node {
        if let Some(&i) = self.index.get(&node.id) {
            return &mut self.nodes[i];
        }
        let i = self.nodes.len();
        let parent = *self.stack.last().expect("root is never popped");
        self.nodes[parent].children.push(node.id);
        self.index.insert(node.id, i);
        self.nodes.push(node);
        &mut self.nodes[i]
    }

    pub fn push(&mut self, id: Id) {
        if let Some(&i) = self.index.get(&id) {
            self.stack.push(i);
        }
    }

    pub fn pop(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    pub fn finish(mut self, focus: Option<Id>) -> Tree {
        let focus = self.focus_override.or(focus).filter(|f| self.index.contains_key(f));
        if let Some(f) = focus {
            let i = self.index[&f];
            self.nodes[i].focused = true;
        }
        Tree { nodes: self.nodes, focus }
    }
}
