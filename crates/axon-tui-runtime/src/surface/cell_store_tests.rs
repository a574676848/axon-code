use super::{CELL_HISTORY_GC_TARGET_CELLS, CELL_HISTORY_MAX_CELLS, CellStoreError, RenderCellList};
use crate::surface::cell::RenderCellRecord;

#[test]
fn replace_active_cell_returns_input_without_touching_committed_history() {
    let mut cells = RenderCellList::default();
    cells.push(RenderCellRecord::agent_message(
        "assistant:test:committed",
        "已提交正文",
    ));
    let replacement =
        RenderCellRecord::streaming_agent_message("assistant:test:stream", "流式正文");

    let replacement = cells
        .replace_active_cell(0, replacement)
        .expect("committed cell must not be replaced as active");

    assert_eq!(
        cells.get(0).map(|cell| cell.text.as_str()),
        Some("已提交正文")
    );
    assert_eq!(replacement.text, "流式正文");
}

#[test]
fn active_cell_updates_refresh_only_its_hot_row() {
    let mut cells = RenderCellList::default();
    for index in 0..128 {
        cells.push(RenderCellRecord::agent_message(
            format!("assistant:test:history:{index}"),
            format!("历史 {index}"),
        ));
    }
    let active = cells.push_active(RenderCellRecord::streaming_agent_message(
        "assistant:test:active",
        "第一个版本",
    ));
    let rebuilds = cells.hot_table_rebuilds();

    for revision in 1..=32 {
        assert!(
            cells
                .replace_active_cell(
                    active,
                    RenderCellRecord::streaming_agent_message(
                        "assistant:test:active",
                        format!("第 {revision} 个版本"),
                    ),
                )
                .is_none()
        );
    }

    assert_eq!(cells.hot_table_rebuilds(), rebuilds);
    assert_eq!(cells.hot.revision[active], 32);
    assert_eq!(
        cells.hot.commit_state[active],
        super::CellCommitState::Active
    );
}

#[test]
fn mutate_cell_only_allows_active_cells_until_commit() {
    let mut cells = RenderCellList::default();
    cells.push(RenderCellRecord::agent_message(
        "assistant:test:committed",
        "committed",
    ));
    assert!(matches!(
        cells.mutate_cell(0, |cell| cell.text = "blocked".to_owned()),
        Err(CellStoreError::Inactive { .. })
    ));

    let active = cells.push_active(RenderCellRecord::streaming_agent_message(
        "assistant:test:active",
        "a",
    ));
    cells
        .mutate_cell(active, |cell| {
            cell.replace_with(RenderCellRecord::streaming_agent_message(
                "assistant:test:active",
                "ab",
            ))
        })
        .expect("active cell should mutate");
    assert_eq!(cells.get(active).map(|cell| cell.text.as_str()), Some("ab"));

    cells
        .commit_cell(active)
        .expect("active cell should commit");
    assert!(matches!(
        cells.mutate_cell(active, |cell| cell.text = "blocked".to_owned()),
        Err(CellStoreError::Inactive { .. })
    ));
}

#[test]
fn transcript_signature_reuses_cache_and_tracks_revision_changes() {
    let mut cells = RenderCellList::default();
    cells.push(RenderCellRecord::agent_message(
        "assistant:test:signature",
        "aaaa",
    ));
    let first = cells.transcript_signature();
    let rebuilds = cells.transcript_signature_rebuilds();

    assert_eq!(cells.transcript_signature(), first);
    assert_eq!(cells.transcript_signature_rebuilds(), rebuilds);

    cells[0].replace_with(RenderCellRecord::agent_message(
        "assistant:test:signature",
        "bbbb",
    ));

    assert_ne!(cells.transcript_signature(), first);
    assert_eq!(
        cells.transcript_signature_rebuilds(),
        rebuilds.saturating_add(1)
    );
}

#[test]
fn history_gc_keeps_recently_accessed_history_before_unread_history() {
    let mut cells = RenderCellList::from_records(
        ["最早历史", "未读取历史", "最新 coding 内容"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                RenderCellRecord::agent_message(format!("assistant:test:lru:{index}"), text)
            })
            .collect(),
    );

    assert_eq!(
        cells.get(0).map(|cell| cell.text.as_str()),
        Some("最早历史")
    );

    let evicted = cells.evict_oldest_history_cells_to(2);

    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].0, 1);
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<Vec<_>>(),
        vec!["最早历史", "最新 coding 内容"]
    );
}

#[test]
fn history_gc_evicts_oldest_committed_cells_in_one_batch() {
    let mut cells = RenderCellList::from_records(
        (0..=CELL_HISTORY_MAX_CELLS)
            .map(|index| {
                RenderCellRecord::agent_message(
                    format!("assistant:test:history:{index}"),
                    format!("历史 {index}"),
                )
            })
            .collect(),
    );
    let active = cells.push_active(RenderCellRecord::streaming_agent_message(
        "assistant:test:running",
        "运行中",
    ));
    let signature_rebuilds = cells.transcript_signature_rebuilds();
    let hot_table_rebuilds = cells.hot_table_rebuilds();

    let evicted = cells.evict_oldest_history_cells_to(CELL_HISTORY_GC_TARGET_CELLS);

    let expected_evictions = active.saturating_add(1) - CELL_HISTORY_GC_TARGET_CELLS;
    assert_eq!(evicted.len(), expected_evictions);
    assert_eq!(cells.len(), CELL_HISTORY_GC_TARGET_CELLS);
    assert_eq!(
        evicted.first().map(|(index, _)| *index),
        Some(0),
        "最旧 committed cell 必须先被驱逐"
    );
    assert_eq!(
        evicted.last().map(|(index, _)| *index),
        Some(expected_evictions - 1)
    );
    assert_eq!(
        cells.get(0).map(|cell| cell.text.as_str()),
        Some(format!("历史 {expected_evictions}").as_str())
    );
    assert_eq!(
        cells
            .get(active - expected_evictions)
            .map(|cell| cell.text.as_str()),
        Some("运行中")
    );
    assert_eq!(
        cells.transcript_signature_rebuilds(),
        signature_rebuilds.saturating_add(1)
    );
    assert_eq!(
        cells.hot_table_rebuilds(),
        hot_table_rebuilds.saturating_add(1)
    );
}
