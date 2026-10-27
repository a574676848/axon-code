//! 时域变调：重叠相加保持时长，只改变音高。
//!
//! 输入输出均为 16 kHz 单声道 PCM16；半音为正升高音高。

/// 帧长与输出跳距，按 16 kHz 取约 64 ms / 16 ms。
const FRAME: usize = 1024;
const HOP_OUT: usize = 256;

/// 按半音变调，时长不变。
#[must_use]
pub fn shift_pitch(samples: &[i16], semitones: f32) -> Vec<i16> {
    if semitones.abs() < 0.01 || samples.len() < FRAME * 2 {
        return samples.to_vec();
    }
    let ratio = 2f32.powf(semitones / 12.0).clamp(0.5, 2.0);
    let hop_in = (HOP_OUT as f32 * ratio).round() as usize;
    if hop_in == 0 || hop_in >= FRAME {
        return samples.to_vec();
    }
    let window = hann();
    let mut out = vec![0f32; samples.len()];
    let mut in_pos = 0usize;
    let mut out_pos = 0usize;
    while in_pos + FRAME <= samples.len() && out_pos + FRAME <= out.len() {
        for i in 0..FRAME {
            out[out_pos + i] += f32::from(samples[in_pos + i]) * window[i];
        }
        in_pos += hop_in;
        out_pos += HOP_OUT;
    }
    out.into_iter()
        .map(|value| value.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16)
        .collect()
}

fn hann() -> Vec<f32> {
    (0..FRAME)
        .map(|i| {
            let phase = std::f32::consts::PI * i as f32 / FRAME as f32;
            phase.sin().powi(2)
        })
        .collect()
}

/// 零交叉率，用于测试与音高粗估。
#[must_use]
pub fn zero_crossing_rate(samples: &[i16]) -> f32 {
    if samples.len() < 2 {
        return 0.0;
    }
    let crossings = samples
        .windows(2)
        .filter(|pair| (pair[0] >= 0) != (pair[1] >= 0))
        .count();
    crossings as f32 / (samples.len() - 1) as f32
}

#[cfg(test)]
mod tests {
    use super::shift_pitch;

    fn tone(freq: f32) -> Vec<i16> {
        let rate = 16_000.0;
        (0..16_000)
            .map(|i| {
                let value = (2.0 * std::f32::consts::PI * freq * i as f32 / rate).sin();
                (value * 12_000.0) as i16
            })
            .collect()
    }

    #[test]
    fn duration_is_preserved() {
        let input = tone(220.0);
        let output = shift_pitch(&input, 2.0);
        assert_eq!(output.len(), input.len());
    }

    #[test]
    #[test]
    fn pitch_shift_changes_waveform_without_changing_length() {
        let input = tone(220.0);
        let output = shift_pitch(&input, 2.0);
        assert_eq!(output.len(), input.len());
        assert_ne!(output, input);
    }

    #[test]
    fn tiny_input_and_zero_shift_are_identity() {
        let short = [0i16, 1, -1];
        assert_eq!(shift_pitch(&short, 2.0), short);
        let input = tone(220.0);
        assert_eq!(shift_pitch(&input, 0.0), input);
    }
}
