//! Undo/redo journal for file operations. Undo/redo actions are themselves
//! executed through the operation queue so they get progress + conflict
//! handling for free.

use crate::queue::{Operation, UndoAction};

struct HistoryEntry {
    undo: UndoAction,
    redo_ops: Vec<Operation>,
}

pub struct UndoJournal {
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    max_depth: usize,
}

impl Default for UndoJournal {
    fn default() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_depth: 64,
        }
    }
}

impl UndoJournal {
    /// Record a user-originated action. Clears the redo stack.
    pub fn record(&mut self, action: UndoAction, redo_ops: Vec<Operation>) {
        self.undo_stack.push(HistoryEntry {
            undo: action,
            redo_ops,
        });
        if self.undo_stack.len() > self.max_depth {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_label(&self) -> Option<String> {
        self.undo_stack.last().map(|e| label_undo(&e.undo))
    }

    pub fn redo_label(&self) -> Option<String> {
        self.redo_stack.last().map(|e| match e.undo {
            UndoAction::DeletePaths(ref p) => format!("Redo create/copy ({} items)", p.len()),
            UndoAction::MoveBack { ref pairs } => format!("Redo move ({} items)", pairs.len()),
            UndoAction::RenameBack { .. } => "Redo rename".to_string(),
        })
    }

    /// Move the latest undo onto the redo stack and return inverse operations.
    pub fn pop_undo(&mut self) -> Vec<Operation> {
        let Some(entry) = self.undo_stack.pop() else {
            return Vec::new();
        };
        let ops = undo_to_ops(&entry.undo);
        self.redo_stack.push(entry);
        ops
    }

    /// Move the latest redo onto the undo stack and return the original ops.
    pub fn pop_redo(&mut self) -> Vec<Operation> {
        let Some(entry) = self.redo_stack.pop() else {
            return Vec::new();
        };
        let ops = entry.redo_ops.clone();
        self.undo_stack.push(entry);
        ops
    }
}

fn label_undo(action: &UndoAction) -> String {
    match action {
        UndoAction::DeletePaths(p) => format!("Undo create/copy ({} items)", p.len()),
        UndoAction::MoveBack { pairs } => format!("Undo move ({} items)", pairs.len()),
        UndoAction::RenameBack { .. } => "Undo rename".to_string(),
    }
}

fn undo_to_ops(action: &UndoAction) -> Vec<Operation> {
    match action {
        UndoAction::DeletePaths(paths) => {
            vec![Operation::Delete {
                paths: paths.clone(),
                recycle: true,
            }]
        }
        UndoAction::MoveBack { pairs } => pairs
            .iter()
            .filter_map(|(orig, moved)| {
                orig.parent().map(|dest| Operation::Move {
                    sources: vec![moved.clone()],
                    dest_dir: dest.to_path_buf(),
                })
            })
            .collect(),
        UndoAction::RenameBack { from, to } => vec![Operation::Rename {
            from: from.clone(),
            to: to.clone(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn undo_then_redo_restores_forward_ops() {
        let mut j = UndoJournal::default();
        let forward = Operation::Copy {
            sources: vec![PathBuf::from(r"C:\a.txt")],
            dest_dir: PathBuf::from(r"D:\"),
        };
        j.record(
            UndoAction::DeletePaths(vec![PathBuf::from(r"D:\a.txt")]),
            vec![forward.clone()],
        );
        assert!(j.can_undo());
        assert!(!j.can_redo());
        let undo_ops = j.pop_undo();
        assert!(matches!(undo_ops[0], Operation::Delete { recycle: true, .. }));
        assert!(j.can_redo());
        let redo_ops = j.pop_redo();
        assert!(matches!(redo_ops[0], Operation::Copy { .. }));
        assert!(j.can_undo());
        assert!(!j.can_redo());
    }

    #[test]
    fn user_record_clears_redo() {
        let mut j = UndoJournal::default();
        j.record(
            UndoAction::DeletePaths(vec![PathBuf::from(r"D:\a.txt")]),
            vec![],
        );
        let _ = j.pop_undo();
        assert!(j.can_redo());
        j.record(
            UndoAction::DeletePaths(vec![PathBuf::from(r"D:\b.txt")]),
            vec![],
        );
        assert!(!j.can_redo());
    }
}
