//! Domain model for SimpleCommander: filesystem entries, directory
//! snapshots, sorting/filtering, and pane/tab state. This crate is
//! platform-agnostic and has no I/O.

pub mod entry;
pub mod query;
pub mod snapshot;
pub mod sort;
pub mod state;

pub use entry::FsEntry;
pub use snapshot::DirSnapshot;
