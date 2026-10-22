use super::*;

#[test]
fn mention_elements_project_only_when_text_still_matches() {
    let mut draft = DraftState::default();
    draft.replace_all("zzzzc$review".to_owned());
    draft.insert_mention(
        5,
        "$review",
        '$',
        "review".to_owned(),
        "skill://review".to_owned(),
    );

    let elements = draft.input_elements("zzzzc$review");

    assert_eq!(elements.len(), 1);
    let snapshots = draft.textarea.elements.snapshots("zzzzc$review");
    assert_eq!(snapshots[0].id, 0);
    assert_eq!(snapshots[0].range, 5..12);
    assert_eq!(snapshots[0].text, "$review");
    assert_eq!(snapshots[0].visual, DraftElementVisualKind::Mention('$'));
}

#[test]
fn edited_mention_drops_stale_metadata() {
    let mut draft = DraftState::default();
    draft.replace_all("$eview".to_owned());
    draft.insert_mention(
        0,
        "$review",
        '$',
        "review".to_owned(),
        "skill://review".to_owned(),
    );

    let elements = draft.input_elements("$eview");

    assert!(elements.is_empty());
}

#[test]
fn atomic_ranges_cover_cursor_inside_mention() {
    let mut draft = DraftState::default();
    draft.replace_all("a $review b".to_owned());
    draft.insert_mention(
        2,
        "$review",
        '$',
        "review".to_owned(),
        "skill://review".to_owned(),
    );

    assert_eq!(
        draft.textarea.elements.atomic_range_touching_backspace(5),
        Some(2..9)
    );
    assert_eq!(
        draft.textarea.elements.atomic_range_touching_delete(2),
        Some(2..9)
    );
    assert_eq!(
        draft.textarea.elements.atomic_range_containing_cursor(3),
        Some(2..9)
    );
}

#[test]
fn image_attachment_uses_compact_placeholder_and_deletes_atomically() {
    let mut draft = DraftState::default();
    draft.attach_file(
        PathBuf::from("C:/tmp/axon-clipboard-image.png"),
        AgentTuiAttachmentKind::Image,
    );

    assert_eq!(draft.text(), "[Image #1]");
    assert_eq!(draft.input_elements(draft.text()).len(), 1);

    draft.backspace();

    assert!(draft.text().is_empty());
    assert!(draft.input_elements(draft.text()).is_empty());
}

#[test]
fn vertical_movement_keeps_preferred_column_across_short_line() {
    let mut draft = DraftState::default();
    draft.replace_all("abcdef\nx\nabcdef".to_owned());
    draft.set_cursor(5);

    draft.move_cursor_down(80);
    assert_eq!(draft.cursor(), 8);

    draft.move_cursor_down(80);
    assert_eq!(draft.cursor(), 14);
}

#[test]
fn vertical_movement_tracks_cjk_display_width_across_soft_wraps() {
    let mut draft = DraftState::default();
    draft.replace_all("你好ab世界cd".to_owned());
    draft.set_cursor(3);

    draft.move_cursor_down(6);
    assert_eq!(draft.visual_cursor_row_col(6), (1, 5));

    draft.move_cursor_up(6);
    assert_eq!(draft.visual_cursor_row_col(6), (0, 5));
}

#[test]
fn horizontal_movement_resets_preferred_column() {
    let mut draft = DraftState::default();
    draft.replace_all("abcdef\nx\nabcdef".to_owned());
    draft.set_cursor(5);

    draft.move_cursor_down(80);
    draft.move_cursor_left();
    draft.move_cursor_down(80);

    assert_eq!(draft.cursor(), 9);
}

#[test]
fn external_edit_keeps_only_attachments_still_present_in_text() {
    let mut draft = DraftState::default();
    draft.attach_file(
        PathBuf::from("C:/tmp/axon-clipboard-image.png"),
        AgentTuiAttachmentKind::Image,
    );
    draft.insert_str(" 说明");

    draft.apply_external_edit("前缀 [Image #1] 后缀".to_owned());

    let elements = draft.element_snapshots();
    assert_eq!(draft.text(), "前缀 [Image #1] 后缀");
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0].visual, DraftElementVisualKind::Image);

    draft.apply_external_edit("附件已删除".to_owned());
    assert!(draft.input_elements(draft.text()).is_empty());
}
