//! 非运行期 transcript surface 的活动索引。

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub(crate) struct TranscriptSurfaceCells {
    command_status_cell: Option<usize>,
    resume_replay_cell: Option<usize>,
    catalog_cells: HashMap<String, usize>,
    local_history_cells: HashMap<String, usize>,
}

impl TranscriptSurfaceCells {
    pub(crate) fn command_status_cell(&self) -> Option<usize> {
        self.command_status_cell
    }

    pub(crate) fn set_command_status_cell(&mut self, index: usize) {
        self.command_status_cell = Some(index);
    }

    pub(crate) fn resume_replay_cell(&self) -> Option<usize> {
        self.resume_replay_cell
    }

    pub(crate) fn set_resume_replay_cell(&mut self, index: usize) {
        self.resume_replay_cell = Some(index);
    }

    pub(crate) fn catalog_cell(&self, key: &str) -> Option<usize> {
        self.catalog_cells.get(key).copied()
    }

    pub(crate) fn set_catalog_cell(&mut self, key: String, index: usize) {
        self.catalog_cells.insert(key, index);
    }

    pub(crate) fn local_history_cell(&self, key: &str) -> Option<usize> {
        self.local_history_cells.get(key).copied()
    }

    pub(crate) fn set_local_history_cell(&mut self, key: String, index: usize) {
        self.local_history_cells.insert(key, index);
    }

    pub(crate) fn remove_transcript_index(&mut self, removed_index: usize) {
        self.command_status_cell =
            shift_index_after_remove(self.command_status_cell, removed_index);
        self.resume_replay_cell = shift_index_after_remove(self.resume_replay_cell, removed_index);
        rebase_map_after_remove(&mut self.catalog_cells, removed_index);
        rebase_map_after_remove(&mut self.local_history_cells, removed_index);
    }
}

fn rebase_map_after_remove(cells: &mut HashMap<String, usize>, removed_index: usize) {
    cells.retain(|_, index| {
        if *index == removed_index {
            return false;
        }
        if *index > removed_index {
            *index -= 1;
        }
        true
    });
}

fn shift_index_after_remove(index: Option<usize>, removed_index: usize) -> Option<usize> {
    let index = index?;
    if index == removed_index {
        None
    } else if index > removed_index {
        Some(index - 1)
    } else {
        Some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::TranscriptSurfaceCells;

    #[test]
    fn remove_transcript_index_rebases_surface_cells() {
        let mut cells = TranscriptSurfaceCells::default();
        cells.set_command_status_cell(5);
        cells.set_resume_replay_cell(7);
        cells.set_catalog_cell("mcp".to_owned(), 6);
        cells.set_local_history_cell("input".to_owned(), 2);

        cells.remove_transcript_index(3);

        assert_eq!(cells.command_status_cell(), Some(4));
        assert_eq!(cells.resume_replay_cell(), Some(6));
        assert_eq!(cells.catalog_cell("mcp"), Some(5));
        assert_eq!(cells.local_history_cell("input"), Some(2));
    }

    #[test]
    fn remove_transcript_index_drops_matching_surface_cells() {
        let mut cells = TranscriptSurfaceCells::default();
        cells.set_command_status_cell(1);
        cells.set_resume_replay_cell(1);
        cells.set_catalog_cell("mcp".to_owned(), 2);
        cells.set_local_history_cell("input".to_owned(), 3);

        cells.remove_transcript_index(1);
        assert_eq!(cells.command_status_cell(), None);
        assert_eq!(cells.resume_replay_cell(), None);
        assert_eq!(cells.catalog_cell("mcp"), Some(1));
        assert_eq!(cells.local_history_cell("input"), Some(2));
    }
}
