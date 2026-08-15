//! Background file-operation engine: queued copy/move/delete with
//! progress, pause/resume, conflict resolution, and an undo journal.

pub mod queue;
pub mod undo;
