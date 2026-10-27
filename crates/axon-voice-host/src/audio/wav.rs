//! WAV 解析：只接受 16 kHz 单声道 PCM16，输出规范化的 s16le 样本。

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavClip {
    /// 16 kHz 单声道样本。
    pub samples: Vec<i16>,
}

impl WavClip {
    /// 时长（毫秒，向下取整）。
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        (self.samples.len() as u64) * 1_000 / 16_000
    }
}

/// WAV 错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavError {
    /// 文件过短或头不完整。
    TooShort,
    /// 不是 RIFF/WAVE。
    NotWave,
    /// 缺少 fmt 或 data 块。
    MissingChunk,
    /// 不是 PCM。
    NotPcm,
    /// 不是 16 位。
    Not16Bit,
    /// 不是单声道。
    NotMono,
    /// 采样率不是 16 kHz。
    WrongRate,
    /// data 块长度为奇数或越界。
    BadData,
    /// 没有样本。
    Empty,
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::TooShort => "文件过短",
            Self::NotWave => "不是 RIFF/WAVE",
            Self::MissingChunk => "缺少 fmt 或 data 块",
            Self::NotPcm => "不是 PCM 编码",
            Self::Not16Bit => "不是 16 位采样",
            Self::NotMono => "不是单声道",
            Self::WrongRate => "采样率不是 16 kHz",
            Self::BadData => "data 块长度非法",
            Self::Empty => "没有样本",
        };
        f.write_str(message)
    }
}

impl std::error::Error for WavError {}

/// 解析 WAV。
///
/// # Errors
///
/// 见 [`WavError`]。
pub fn parse(bytes: &[u8]) -> Result<WavClip, WavError> {
    if bytes.len() < 12 {
        return Err(WavError::TooShort);
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }
    let mut offset = 12;
    let mut format: Option<(u16, u16, u32)> = None;
    let mut data: Option<&[u8]> = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let body_start = offset + 8;
        let body_end = body_start.checked_add(size).ok_or(WavError::BadData)?;
        if body_end > bytes.len() {
            return Err(WavError::BadData);
        }
        let body = &bytes[body_start..body_end];
        if id == b"fmt " {
            if body.len() < 16 {
                return Err(WavError::TooShort);
            }
            let format_tag = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let sample_rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            let bits = u16::from_le_bytes([body[14], body[15]]);
            format = Some((format_tag, channels, sample_rate | (u32::from(bits) << 16)));
        } else if id == b"data" {
            data = Some(body);
        }
        offset = body_end + (size % 2);
    }

    let Some((format_tag, channels, packed)) = format else {
        return Err(WavError::MissingChunk);
    };
    let sample_rate = packed & 0xffff;
    let bits = (packed >> 16) as u16;
    if format_tag != 1 {
        return Err(WavError::NotPcm);
    }
    if bits != 16 {
        return Err(WavError::Not16Bit);
    }
    if channels != 1 {
        return Err(WavError::NotMono);
    }
    if sample_rate != 16_000 {
        return Err(WavError::WrongRate);
    }
    let data = data.ok_or(WavError::MissingChunk)?;
    if data.is_empty() {
        return Err(WavError::Empty);
    }
    if !data.len().is_multiple_of(2) {
        return Err(WavError::BadData);
    }
    let samples = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect();
    Ok(WavClip { samples })
}

#[cfg(test)]
mod tests {
    use super::{WavError, parse};

    fn wav(channels: u16, rate: u32, bits: u16, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn parses_canonical_clip() {
        let clip = parse(&wav(1, 16_000, 16, &[0, 1, -2])).expect("应解析");
        assert_eq!(clip.samples, vec![0, 1, -2]);
        assert_eq!(clip.duration_ms(), 0);
    }

    #[test]
    fn rejects_non_canonical_inputs() {
        assert_eq!(
            parse(wav(2, 16_000, 16, &[0, 0]).as_slice()),
            Err(WavError::NotMono)
        );
        assert_eq!(
            parse(wav(1, 44_100, 16, &[0]).as_slice()),
            Err(WavError::WrongRate)
        );
        assert_eq!(
            parse(wav(1, 16_000, 8, &[0]).as_slice()),
            Err(WavError::Not16Bit)
        );
        assert_eq!(parse(b"not a wav file").unwrap_err(), WavError::NotWave);
        assert_eq!(parse(&[]).unwrap_err(), WavError::TooShort);
    }

    #[test]
    fn empty_data_is_rejected() {
        assert_eq!(
            parse(wav(1, 16_000, 16, &[]).as_slice()),
            Err(WavError::Empty)
        );
    }

    #[test]
    fn truncated_data_is_rejected() {
        let mut bytes = wav(1, 16_000, 16, &[0, 1]);
        bytes.truncate(bytes.len() - 1);
        assert_eq!(parse(&bytes).unwrap_err(), WavError::BadData);
    }
}
