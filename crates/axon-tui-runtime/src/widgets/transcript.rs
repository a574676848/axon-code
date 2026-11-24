//! 聊天记录 Transcript Widget，支持富文本渲染、自动换行和主渲染区分页。

#[cfg(test)]
use crate::widgets::transcript_viewport::transcript_viewport_metrics;
use crate::widgets::{
    TranscriptViewportMetrics,
    transcript_viewport::{
        TranscriptViewportRequest, transcript_viewport_metrics_with_height_cache,
    },
};
#[cfg(test)]
use crate::{
    surface::{cell_store::RenderCellList, height_index::LiveTranscriptTail},
    widgets::TranscriptRenderCache,
};

#[cfg(test)]
pub(crate) fn transcript_view_metrics(
    lines: &RenderCellList,
    height: u16,
    width: u16,
    selected_cell: Option<usize>,
    selected_line: Option<usize>,
    active_tail: Option<LiveTranscriptTail<'_>>,
    cache: &TranscriptRenderCache,
) -> TranscriptViewportMetrics {
    transcript_viewport_metrics(
        lines,
        height,
        width,
        selected_cell,
        selected_line,
        active_tail,
        cache,
    )
}

pub(crate) fn transcript_view_metrics_with_height_cache(
    request: TranscriptViewportRequest<'_>,
) -> TranscriptViewportMetrics {
    transcript_viewport_metrics_with_height_cache(request)
}
