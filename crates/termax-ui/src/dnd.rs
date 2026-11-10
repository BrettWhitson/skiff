//! Drag-and-drop between widgets: a source calls `Ui::start_drag` with any
//! payload, targets call `Ui::drop_target::<T>(rect)` every frame and get
//! the payload when the left button is released over them.

use std::any::Any;

pub(crate) struct DragState {
    pub payload: Box<dyn Any>,
    pub label: String,
}

#[derive(Debug)]
pub struct DropResult<T> {
    /// A drag carrying a `T` is over this target.
    pub hovered: bool,
    /// The payload, on the frame it is dropped here.
    pub dropped: Option<T>,
}

impl<T> Default for DropResult<T> {
    fn default() -> Self {
        DropResult { hovered: false, dropped: None }
    }
}
