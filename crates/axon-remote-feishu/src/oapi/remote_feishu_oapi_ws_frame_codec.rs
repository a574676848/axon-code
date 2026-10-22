//! Feishu OAPI WebSocket pbbp2 frame 编解码。

use axon_domain::{AxonError, AxonResult};

use crate::remote_feishu_oapi_ws_sdk::{RemoteFeishuOapiWsFrame, RemoteFeishuOapiWsHeader};

const VARINT_CONTINUATION_BIT: u8 = 0x80;
const VARINT_VALUE_MASK: u64 = 0x7f;
const VARINT_FINAL_SHIFT: u32 = 63;
const VARINT_MAX_SHIFT: u32 = 64;
const VARINT_FINAL_VALUE_MAX: u8 = 1;
pub(crate) const WS_FRAME_MAX_HEADERS: usize = 128;

/// 编码 pbbp2 frame。
///
/// # Errors
///
/// 当字段长度超出 protobuf varint 编码范围时返回错误。
pub fn encode_frame(frame: &RemoteFeishuOapiWsFrame) -> AxonResult<Vec<u8>> {
    if frame.headers.len() > WS_FRAME_MAX_HEADERS {
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS frame header 数量超出上限",
        ));
    }
    let mut output = Vec::new();
    write_varint_field(&mut output, 1, frame.seq_id);
    write_varint_field(&mut output, 2, frame.log_id);
    write_varint_field(&mut output, 3, signed_to_u64(frame.service)?);
    write_varint_field(&mut output, 4, signed_to_u64(frame.method)?);
    for header in &frame.headers {
        let mut header_bytes = Vec::new();
        write_bytes_field(&mut header_bytes, 1, header.key.as_bytes())?;
        write_bytes_field(&mut header_bytes, 2, header.value.as_bytes())?;
        write_bytes_field(&mut output, 5, &header_bytes)?;
    }
    write_bytes_field(&mut output, 6, frame.payload_encoding.as_bytes())?;
    write_bytes_field(&mut output, 7, frame.payload_type.as_bytes())?;
    if !frame.payload.is_empty() {
        write_bytes_field(&mut output, 8, &frame.payload)?;
    }
    write_bytes_field(&mut output, 9, frame.log_id_new.as_bytes())?;
    Ok(output)
}

/// 解码 pbbp2 frame。
///
/// # Errors
///
/// 当 protobuf wire 数据无效时返回错误。
pub fn decode_frame(bytes: &[u8]) -> AxonResult<RemoteFeishuOapiWsFrame> {
    let mut frame = RemoteFeishuOapiWsFrame::default();
    let mut index = 0usize;
    while index < bytes.len() {
        let key = read_varint(bytes, &mut index)?;
        let field = key >> 3;
        let wire = key & 0x07;
        match (field, wire) {
            (1, 0) => frame.seq_id = read_varint(bytes, &mut index)?,
            (2, 0) => frame.log_id = read_varint(bytes, &mut index)?,
            (3, 0) => frame.service = varint_to_i32(read_varint(bytes, &mut index)?)?,
            (4, 0) => frame.method = varint_to_i32(read_varint(bytes, &mut index)?)?,
            (5, 2) => {
                if frame.headers.len() >= WS_FRAME_MAX_HEADERS {
                    return Err(AxonError::runtime_failure(
                        "Feishu OAPI WS frame header 数量超出上限",
                    ));
                }
                frame
                    .headers
                    .push(decode_header(read_len(bytes, &mut index)?)?);
            }
            (6, 2) => frame.payload_encoding = decode_string(read_len(bytes, &mut index)?)?,
            (7, 2) => frame.payload_type = decode_string(read_len(bytes, &mut index)?)?,
            (8, 2) => frame.payload = read_len(bytes, &mut index)?.to_vec(),
            (9, 2) => frame.log_id_new = decode_string(read_len(bytes, &mut index)?)?,
            (_, 0) => {
                let _ = read_varint(bytes, &mut index)?;
            }
            (_, 2) => {
                let _ = read_len(bytes, &mut index)?;
            }
            _ => {
                return Err(AxonError::runtime_failure(
                    "Feishu OAPI WS frame wire type 无效",
                ));
            }
        }
    }
    Ok(frame)
}

fn decode_header(bytes: &[u8]) -> AxonResult<RemoteFeishuOapiWsHeader> {
    let mut header = RemoteFeishuOapiWsHeader::default();
    let mut index = 0usize;
    while index < bytes.len() {
        let key = read_varint(bytes, &mut index)?;
        let field = key >> 3;
        let wire = key & 0x07;
        match (field, wire) {
            (1, 2) => header.key = decode_string(read_len(bytes, &mut index)?)?,
            (2, 2) => header.value = decode_string(read_len(bytes, &mut index)?)?,
            (_, 0) => {
                let _ = read_varint(bytes, &mut index)?;
            }
            (_, 2) => {
                let _ = read_len(bytes, &mut index)?;
            }
            _ => {
                return Err(AxonError::runtime_failure(
                    "Feishu OAPI WS header wire type 无效",
                ));
            }
        }
    }
    Ok(header)
}

fn write_varint_field(output: &mut Vec<u8>, field: u64, value: u64) {
    write_varint(output, field << 3);
    write_varint(output, value);
}

fn write_bytes_field(output: &mut Vec<u8>, field: u64, value: &[u8]) -> AxonResult<()> {
    write_varint(output, (field << 3) | 2);
    write_varint(output, usize_to_u64(value.len())?);
    output.extend_from_slice(value);
    Ok(())
}

fn write_varint(output: &mut Vec<u8>, mut value: u64) {
    while value >= u64::from(VARINT_CONTINUATION_BIT) {
        output.push(low_7_bits(value) | VARINT_CONTINUATION_BIT);
        value >>= 7;
    }
    output.push(low_7_bits(value));
}

fn low_7_bits(value: u64) -> u8 {
    u8::try_from(value & VARINT_VALUE_MASK).unwrap_or(0)
}

fn read_varint(bytes: &[u8], index: &mut usize) -> AxonResult<u64> {
    let mut shift = 0u32;
    let mut value = 0u64;
    while *index < bytes.len() {
        let byte = bytes[*index];
        *index += 1;
        let value_bits = byte & 0x7f;
        if shift == VARINT_FINAL_SHIFT && value_bits > VARINT_FINAL_VALUE_MAX {
            return Err(AxonError::runtime_failure(
                "Feishu OAPI WS protobuf varint 溢出",
            ));
        }
        value |= u64::from(value_bits) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= VARINT_MAX_SHIFT {
            break;
        }
    }
    Err(AxonError::runtime_failure(
        "Feishu OAPI WS protobuf varint 无效",
    ))
}

fn read_len<'a>(bytes: &'a [u8], index: &mut usize) -> AxonResult<&'a [u8]> {
    let len = usize::try_from(read_varint(bytes, index)?)
        .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS protobuf 长度无效"))?;
    let end = index.checked_add(len).ok_or(AxonError::runtime_failure(
        "Feishu OAPI WS protobuf 长度溢出",
    ))?;
    if end > bytes.len() {
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS protobuf 长度越界",
        ));
    }
    let slice = &bytes[*index..end];
    *index = end;
    Ok(slice)
}

fn decode_string(bytes: &[u8]) -> AxonResult<String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS protobuf 字符串无效"))
}

fn signed_to_u64(value: i32) -> AxonResult<u64> {
    u64::try_from(value)
        .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS protobuf int32 无效"))
}

fn varint_to_i32(value: u64) -> AxonResult<i32> {
    i32::try_from(value)
        .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS protobuf int32 越界"))
}

fn usize_to_u64(value: usize) -> AxonResult<u64> {
    u64::try_from(value).map_err(|_| AxonError::runtime_failure("Feishu OAPI WS protobuf 长度越界"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_terminal_varint_overflow() {
        let mut bytes =
            vec![VARINT_CONTINUATION_BIT; usize::try_from(VARINT_FINAL_SHIFT / 7).unwrap_or(0)];
        bytes.push(VARINT_FINAL_VALUE_MAX + 1);

        let mut index = 0;
        let error = read_varint(&bytes, &mut index).expect_err("溢出 varint 必须被拒绝");

        assert!(format!("{error:?}").contains("varint 溢出"));
    }
}
