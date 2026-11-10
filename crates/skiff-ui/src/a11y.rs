//! The accessibility tree: widgets describe themselves (role, name, state)
//! while they draw, and the shell hands the result to the platform screen
//! reader through AccessKit (see `a11y_kit`). Building is skipped entirely
//! unless an assistive technology asked for the tree.

use std::collections::HashMap;

use crate::geom::Rect;
use crate::ui::Id;

/// The window root's id (stable across frames).
pub const ROOT: Id = Id(1);

/// What kind of control a node is; mapped one-to-one onto AccessKit roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// The top-level window (always the tree root).
    Window,
    Group,
    Region,
    Button,
    CheckBox,
    TextInput,
    /// A text input whose contents are masked.
    PasswordInput,
    List,
    ListItem,
    TabList,
    Tab,
    TabPanel,
    Menu,
    MenuItem,
    /// A dropdown that opens a list of choices.
    ComboBox,
    Label,
    /// A terminal emulator view.
    Terminal,
    Dialog,
    /// A status line whose changes are worth announcing.
    Status,
    /// A draggable divider between two panes (value: the first pane's share).
    Splitter,
}

/// One accessible element as a widget described it this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The widget's id; also the AccessKit node id.
    pub id: Id,
    pub role: Role,
    /// The accessible label read out by the screen reader.
    pub name: String,
    /// The current value (text contents, selected choice, splitter share).
    pub value: Option<String>,
    /// Physical pixels.
    pub bounds: Rect,
    /// Set by `Builder::finish` on the node that has keyboard focus.
    pub focused: bool,
    /// `Some` only for selectable items (rows, tabs).
    pub selected: Option<bool>,
    /// `Some` only for toggles (checkboxes).
    pub checked: Option<bool>,
    /// `Some` only for things that open and close (dropdowns, menus).
    pub expanded: Option<bool>,
    pub disabled: bool,
    /// Changes are announced politely (status text, terminal output).
    pub live: bool,
    /// Child node ids, in drawing order.
    pub children: Vec<Id>,
}

impl Node {
    /// A node with the given identity and every optional state unset.
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
    /// All nodes, root first, in the order widgets added them.
    pub nodes: Vec<Node>,
    /// The focused node, if it was added this frame.
    pub focus: Option<Id>,
}

impl Tree {
    /// The first node named exactly `name`.
    pub fn find(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }

    /// The node with id `id`, if present (a linear scan).
    pub fn by_id(&self, id: Id) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// Appended to a live region's text to tell a repeat from no change.
pub const REPEAT_MARK: char = '\u{200B}';

/// The new text for a live region that showed `prev`. Screen readers speak
/// a live region only when its text changes, so when `next` reads the same
/// as `prev` a zero-width marker is toggled on or off: the repeat is spoken
/// and nothing audible is added.
pub fn fresh_announcement(prev: &str, mut next: String) -> String {
    // Only add the mark when `prev` lacks it; if `prev` had it, plain `next`
    // already differs, which toggles the mark off.
    if prev.trim_end_matches(REPEAT_MARK) == next && !prev.ends_with(REPEAT_MARK) {
        next.push(REPEAT_MARK);
    }
    next
}

/// What a screen reader asks a widget to do.
#[derive(Debug, Clone, PartialEq)]
pub enum A11yAction {
    /// Activate (press a button, pick a row or tab).
    Click,
    /// Move keyboard focus to the widget.
    Focus,
    /// Replace a text input's contents.
    SetValue(String),
    /// Step a value up (a splitter moves right/down).
    Increment,
    /// Step a value down.
    Decrement,
    ScrollIntoView,
}

/// Collects nodes during a frame.
pub(crate) struct Builder {
    /// Nodes added so far; index 0 is the root.
    nodes: Vec<Node>,
    /// Node id to its position in `nodes`, for de-duplication and lookups.
    index: HashMap<Id, usize>,
    /// Indices of the open parents; new nodes attach to the last one.
    stack: Vec<usize>,
    /// A widget that represents focus by a child (a list's selected row).
    pub focus_override: Option<Id>,
}

impl Builder {
    /// Starts a frame's tree with just the window root, which is the
    /// current parent.
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

    /// Makes the already-added node `id` the parent for following nodes;
    /// an unknown id is ignored (and so must not be popped).
    pub fn push(&mut self, id: Id) {
        if let Some(&i) = self.index.get(&id) {
            self.stack.push(i);
        }
    }

    /// Returns to the previous parent; the root is never popped.
    pub fn pop(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    /// Produces the frame's tree, marking the focused node. A focus override
    /// wins over `focus`; a focus target that drew no node is dropped.
    pub fn finish(mut self, focus: Option<Id>) -> Tree {
        let focus = self.focus_override.or(focus).filter(|f| self.index.contains_key(f));
        if let Some(f) = focus {
            let i = self.index[&f];
            self.nodes[i].focused = true;
        }
        Tree { nodes: self.nodes, focus }
    }
}
