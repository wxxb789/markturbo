//! Headless filesystem and navigation state for an opened workspace.
//!
//! These modules share the workspace's tree, search, walk policy, watcher, and
//! UI-independent tab/history state without depending on `mt-app` or GPUI.

pub mod history;
pub mod search;
pub mod tabs;
pub mod tree;
pub mod walk;
pub mod watcher;

pub use history::{History, Visit};
pub use tabs::{Tab, TabIdentity, Tabs};
pub use tree::{FileNode, display_relative, is_openable, read_dir, read_dir_deep};
pub use watcher::{Change, Watcher};
