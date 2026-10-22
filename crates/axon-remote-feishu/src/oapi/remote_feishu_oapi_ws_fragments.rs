use std::collections::HashMap;

use axon_domain::{AxonError, AxonResult};

use super::{
    FRAME_TYPE_DATA, HEADER_MESSAGE_ID, HEADER_SEQ, HEADER_SUM, RemoteFeishuOapiWsFrame,
    WS_FRAGMENT_MAX_BUFFERED_BYTES, WS_FRAGMENT_MAX_INFLIGHT_MESSAGES,
    WS_FRAGMENT_MAX_MESSAGE_BYTES, WS_FRAGMENT_MAX_MESSAGE_ID_BYTES, WS_FRAGMENT_MAX_PARTS,
    header_usize, header_value,
};

#[derive(Debug, Default)]
pub(super) struct FragmentBuffer {
    messages: HashMap<String, FragmentAccumulator>,
    total_bytes: usize,
}

#[derive(Debug)]
struct FragmentAccumulator {
    parts: Vec<Option<Vec<u8>>>,
    total_bytes: usize,
}

impl FragmentBuffer {
    pub(super) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    #[cfg(test)]
    pub(super) fn buffered_bytes(&self) -> usize {
        self.total_bytes
    }

    fn discard(&mut self, message_id: &str) -> Option<FragmentAccumulator> {
        let accumulator = self.messages.remove(message_id)?;
        self.total_bytes = self.total_bytes.saturating_sub(accumulator.total_bytes);
        Some(accumulator)
    }
}

pub(super) fn combine_fragmented_frame(
    mut frame: RemoteFeishuOapiWsFrame,
    fragments: &mut FragmentBuffer,
) -> AxonResult<Option<RemoteFeishuOapiWsFrame>> {
    if frame.method != FRAME_TYPE_DATA {
        return Ok(Some(frame));
    }
    let sum = header_usize(&frame.headers, HEADER_SUM).unwrap_or(1);
    if sum <= 1 {
        return Ok(Some(frame));
    }
    if sum > WS_FRAGMENT_MAX_PARTS {
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 分片数量超出上限",
        ));
    }
    let seq = header_usize(&frame.headers, HEADER_SEQ).unwrap_or(0);
    if seq >= sum {
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 分片序号无效",
        ));
    }
    let message_id = header_value(&frame.headers, HEADER_MESSAGE_ID).ok_or(
        AxonError::runtime_failure("Feishu OAPI WS data frame 分片缺少 message_id"),
    )?;
    if message_id.len() > WS_FRAGMENT_MAX_MESSAGE_ID_BYTES {
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 分片 message_id 超出上限",
        ));
    }
    let payload = std::mem::take(&mut frame.payload);
    if payload.len() > WS_FRAGMENT_MAX_MESSAGE_BYTES {
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 分片正文超出上限",
        ));
    }
    if !fragments.messages.contains_key(&message_id)
        && fragments.messages.len() >= WS_FRAGMENT_MAX_INFLIGHT_MESSAGES
    {
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 未完成分片消息数超出上限",
        ));
    }
    let payload_len = payload.len();
    let next_message = {
        let buffer = fragments
            .messages
            .entry(message_id.clone())
            .or_insert_with(|| FragmentAccumulator {
                parts: (0..sum).map(|_| None).collect(),
                total_bytes: 0,
            });
        if buffer.parts.len() == sum {
            let replaced_len = buffer.parts[seq].as_ref().map_or(0, Vec::len);
            let next_message_bytes = buffer
                .total_bytes
                .saturating_sub(replaced_len)
                .checked_add(payload_len)
                .ok_or(AxonError::runtime_failure(
                    "Feishu OAPI WS data frame 分片正文长度溢出",
                ))?;
            Some((replaced_len, next_message_bytes))
        } else {
            None
        }
    };
    let Some((replaced_len, next_message_bytes)) = next_message else {
        fragments.discard(&message_id);
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 分片数量不一致",
        ));
    };
    let next_total_bytes = fragments
        .total_bytes
        .saturating_sub(replaced_len)
        .checked_add(payload_len)
        .ok_or(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 分片缓存长度溢出",
        ))?;
    if next_message_bytes > WS_FRAGMENT_MAX_MESSAGE_BYTES {
        fragments.discard(&message_id);
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 重组正文超出上限",
        ));
    }
    if next_total_bytes > WS_FRAGMENT_MAX_BUFFERED_BYTES {
        fragments.discard(&message_id);
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 未完成分片缓存超出上限",
        ));
    }
    let buffer = fragments
        .messages
        .get_mut(&message_id)
        .ok_or(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 分片缓存缺失",
        ))?;
    buffer.parts[seq] = Some(payload);
    buffer.total_bytes = next_message_bytes;
    let completed = buffer.parts.iter().all(Option::is_some);
    fragments.total_bytes = next_total_bytes;
    if !completed {
        return Ok(None);
    }
    let accumulator = fragments
        .discard(&message_id)
        .ok_or(AxonError::runtime_failure(
            "Feishu OAPI WS data frame 分片缓存缺失",
        ))?;
    frame.payload = Vec::with_capacity(accumulator.total_bytes);
    for part in accumulator.parts.into_iter().flatten() {
        frame.payload.extend(part);
    }
    Ok(Some(frame))
}
