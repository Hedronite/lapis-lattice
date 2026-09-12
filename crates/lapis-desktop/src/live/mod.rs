//! A Markdown display projection. Text and history remain owned by the source editor.
mod projection;
pub use projection::{Bias, Block, Kind, Projection, Span, Style};

#[cfg(feature = "gpui")]
pub mod editor;

#[cfg(feature = "gpui")]
pub use editor::LiveEditor;
