use std::ops::{Index, IndexMut};

use super::{RENDER_CELL_RECORD_CONTRACT_ERROR, RenderCellList, RenderCellRecord};

impl From<Vec<RenderCellRecord>> for RenderCellList {
    fn from(records: Vec<RenderCellRecord>) -> Self {
        Self::from_records(records)
    }
}

impl Index<usize> for RenderCellList {
    type Output = RenderCellRecord;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect(RENDER_CELL_RECORD_CONTRACT_ERROR)
    }
}

impl IndexMut<usize> for RenderCellList {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.get_mut(index)
            .expect(RENDER_CELL_RECORD_CONTRACT_ERROR)
    }
}
