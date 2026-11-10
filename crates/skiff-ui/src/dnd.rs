//! Drag-and-drop between widgets: a source calls `Ui::start_drag` with any
//! payload, targets call `Ui::drop_target::<T>(rect)` every frame and get
//! the payload when the left button is released over them.

use std::any::Any;

/// The drag in progress, kept in `UiState` across frames.
pub(crate) struct DragState {
    /// What is being dragged; targets match on its concrete type.
    pub payload: Box<dyn Any>,
    /// Text drawn next to the cursor while dragging.
    pub label: String,
}

/// What a drop target learns this frame.
#[derive(Debug)]
pub struct DropResult<T> {
    /// A drag carrying a `T` is over this target.
    pub hovered: bool,
    /// The payload, on the frame it is dropped here.
    pub dropped: Option<T>,
}

impl<T> Default for DropResult<T> {
    /// Not hovered, nothing dropped (written by hand so `T` needs no `Default`).
    fn default() -> Self {
        DropResult { hovered: false, dropped: None }
    }
}
