mod geometry;
mod output_id;
mod window_id;

pub use crate::{default_state_dir, default_state_dir_from_home, shell_quote};
pub use geometry::Rect;
pub use output_id::OutputId;
pub(crate) use output_id::OutputIdAllocator;
pub use window_id::WindowId;
