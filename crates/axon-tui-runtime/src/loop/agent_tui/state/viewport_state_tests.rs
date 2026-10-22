//! ViewportState 行为测试。

use super::viewport_state::{
    ColdBottomMountPhase, TranscriptViewBoundsCause, ViewportScrollState, ViewportState,
};
use crate::surface::viewport_anchor::ScrollAnchor;

#[test]
fn runtime_selection_tracks_part_identity_across_handoff() {
    let mut viewport = ViewportState::new();
    viewport.select_runtime_part(7, "reasoning:turn-7".to_owned());

    let handoff = viewport.handoff_state();

    let mut restored = ViewportState::new();
    restored.restore_handoff_state(handoff);

    assert_eq!(restored.selected_runtime_render_index(), Some(7));
    assert_eq!(
        restored.selected_runtime_part_id(),
        Some("reasoning:turn-7")
    );
}

#[test]
fn runtime_selection_clears_when_selected_cell_is_removed() {
    let mut viewport = ViewportState::new();
    viewport.select_runtime_part(7, "reasoning:turn-7".to_owned());

    viewport.rebase_runtime_selection_after_remove(7);

    assert_eq!(viewport.selected_runtime_render_index(), None);
    assert_eq!(viewport.selected_runtime_part_id(), None);
}

#[test]
fn bottom_attached_follows_new_tail() {
    let mut viewport = ViewportState::new();
    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        20,
        10,
        None,
        None,
    );

    assert_eq!(viewport.transcript_view_offset_rows, 0);
    assert_eq!(
        viewport.transcript_view_scroll_state,
        ViewportScrollState::BottomAttached
    );
}

#[test]
fn user_detached_keeps_visible_anchor_when_content_grows() {
    let mut viewport = ViewportState::new();
    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        20,
        10,
        None,
        None,
    );
    viewport.detach_transcript_view_at_offset(12);

    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        25,
        10,
        None,
        None,
    );

    assert_eq!(viewport.transcript_view_offset_rows, 17);
    assert_eq!(
        viewport.transcript_view_scroll_state,
        ViewportScrollState::UserDetached
    );
}

#[test]
fn scrolling_back_to_bottom_reattaches_tail_follow() {
    let mut viewport = ViewportState::new();
    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        20,
        10,
        None,
        None,
    );
    viewport.detach_transcript_view_at_offset(12);

    viewport.detach_transcript_view_at_offset(0);
    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        30,
        10,
        None,
        None,
    );

    assert_eq!(viewport.transcript_view_offset_rows, 0);
    assert_eq!(
        viewport.transcript_view_scroll_state,
        ViewportScrollState::BottomAttached
    );
}

#[test]
fn jump_affordance_appears_only_past_threshold_when_user_detached() {
    let mut viewport = ViewportState::new();
    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        100,
        12,
        None,
        None,
    );

    viewport.detach_transcript_view_at_offset(40);
    assert!(!viewport.transcript_view_jump_affordance().available);

    viewport.detach_transcript_view_at_offset(41);
    let affordance = viewport.transcript_view_jump_affordance();

    assert_eq!(
        viewport.transcript_view_scroll_state,
        ViewportScrollState::JumpAvailable
    );
    assert!(affordance.available);
    assert!(affordance.overflow);
    assert_eq!(affordance.distance_rows, 41);
    assert_eq!(affordance.threshold_rows, 40);
}

#[test]
fn bottom_attached_never_shows_jump_affordance() {
    let mut viewport = ViewportState::new();
    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        100,
        12,
        None,
        None,
    );

    assert_eq!(viewport.transcript_view_offset_rows, 0);
    let affordance = viewport.transcript_view_jump_affordance();
    assert!(affordance.overflow);
    assert!(!affordance.available);
}

#[test]
fn near_bottom_reflow_keeps_user_detached() {
    let mut viewport = ViewportState::new();
    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        40,
        10,
        None,
        None,
    );
    viewport.detach_transcript_view_at_offset(9);

    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ViewportReflow,
        40,
        10,
        None,
        None,
    );

    assert_eq!(viewport.transcript_view_offset_rows, 9);
    assert_eq!(
        viewport.transcript_view_scroll_state,
        ViewportScrollState::UserDetached
    );
}

#[test]
fn resize_restore_maps_detached_anchor_row_to_new_bottom_offset() {
    let mut viewport = ViewportState::new();
    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        90,
        10,
        None,
        None,
    );
    viewport.detach_transcript_view_at_offset(30);
    let anchor = ScrollAnchor {
        cell_id: 7,
        row_offset_in_cell: 2,
    };

    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ViewportReflow,
        120,
        12,
        Some(80),
        Some(anchor),
    );

    assert_eq!(viewport.transcript_view_offset_rows, 40);
    assert_eq!(
        viewport.transcript_view_scroll_state,
        ViewportScrollState::UserDetached
    );
    assert_eq!(viewport.transcript_view_top_anchor, Some(anchor));
}

#[test]
fn resize_restore_keeps_bottom_attached_at_tail() {
    let mut viewport = ViewportState::new();
    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        90,
        10,
        None,
        None,
    );
    let anchor = ScrollAnchor {
        cell_id: 3,
        row_offset_in_cell: 0,
    };

    viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ViewportReflow,
        120,
        12,
        Some(80),
        Some(anchor),
    );

    assert_eq!(viewport.transcript_view_offset_rows, 0);
    assert_eq!(
        viewport.transcript_view_scroll_state,
        ViewportScrollState::BottomAttached
    );
    assert_eq!(viewport.transcript_view_top_anchor, Some(anchor));
}

#[test]
fn cold_bottom_mount_limits_first_long_projection_to_tail_then_expands() {
    let mut viewport = ViewportState::new();

    assert!(viewport.prepare_cold_bottom_mount(32));
    assert_eq!(
        viewport.cold_bottom_mount_phase(),
        ColdBottomMountPhase::TailOnly
    );
    assert_eq!(viewport.active_runtime_projection_start(32), 22);

    assert!(viewport.complete_cold_bottom_mount_frame());
    assert_eq!(
        viewport.cold_bottom_mount_phase(),
        ColdBottomMountPhase::Full
    );
    assert_eq!(viewport.active_runtime_projection_start(32), 0);
    assert!(!viewport.prepare_cold_bottom_mount(32));
    assert_eq!(viewport.active_runtime_projection_start(32), 0);
}

#[test]
fn cold_bottom_mount_keeps_short_projection_full() {
    let mut viewport = ViewportState::new();

    assert!(!viewport.prepare_cold_bottom_mount(10));

    assert_eq!(
        viewport.cold_bottom_mount_phase(),
        ColdBottomMountPhase::NotMounted
    );
    assert_eq!(viewport.active_runtime_projection_start(10), 0);
}
