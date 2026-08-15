//! Undo/redo journal for file operations. Undo actions are themselves
//! executed through the operation queue so they get progress + conflict
//! handling for free.

use crate::queue::{Operation, UndoAction};

pub struct UndoJournal {
    undo_stack: Vec<UndoAction>,
    redo_stack: Vec<UndoAction>,
    max_depth: usize,
}

impl Default for UndoJournal {
    fn default() -> Self {
        Self { undo_stack: Vec::new(), redo_stack: Vec::new(), max_depth: 64 }
    }
}

impl UndoJournal {
    pub fn record(&mut self, action: UndoAction) {
        self.undo_stack.push(action);
        if self.undo_stack.len() > self.max_depth {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn undo_label(&self) -> Option<String> {
        self.undo_stack.last().map(|a| match a {
            UndoAction::DeletePaths(p) => format!("Undo create/copy ({} items)", p.len()),
            UndoAction::MoveBack { pairs } => format!("Undo move ({} items)", pairs.len()),
            UndoAction::RenameBack { .. } => "Undo rename".to_string(),
        })
    }

    /// Pop the next undo action and convert it into operations to run.
    pub fn pop_undo(&mut self) -> Vec<Operation> {
        let Some(action) = self.undo_stack.pop() else {
            return Vec::new();
        };
        match action {
            UndoAction::DeletePaths(paths) => {
                vec![Operation::Delete { paths, recycle: true }]
            }
            UndoAction::MoveBack { pairs } => pairs
                .into_iter()
                .filter_map(|(orig, moved)| {
                    orig.parent().map(|dest| Operation::Move {
                        sources: vec![moved],
                        dest_dir: dest.to_path_buf(),
                    })
                })
                .collect(),
            UndoAction::RenameBack { from, to } => vec![Operation::Rename { from, to }],
        }
    }
}
