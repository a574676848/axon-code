/*
 * 语音采集的纯计算：重采样、s16le 编码与有界帧队列。
 *
 * 协议要求 16 kHz 单声道、每 100 ms 一帧；WebView 的 AudioContext 未必按请求的
 * 采样率启动，因此这里统一做一次线性重采样再合帧。队列有界且丢最旧：网络或宿主
 * 跟不上时，宁可丢掉最早的音频，也不让内存无界增长或把最新的话挤掉。
 */

/** 协议固定的采样率。 */
export const VOICE_SAMPLE_RATE = 16_000;
/** 协议固定的合帧时长。 */
export const VOICE_FRAME_MS = 100;
/** 一帧的采样点数：16000 × 0.1。 */
export const VOICE_FRAME_SAMPLES = (VOICE_SAMPLE_RATE * VOICE_FRAME_MS) / 1_000;
/** 上行队列上限：8 帧即 800 ms。 */
export const MAX_QUEUED_FRAMES = 8;

/** 线性插值重采样；相等采样率时原样返回，不做无谓拷贝。 */
export function resampleLinear(
  input: Float32Array,
  fromRate: number,
  toRate: number,
): Float32Array {
  if (input.length === 0 || fromRate <= 0 || toRate <= 0) return new Float32Array(0);
  if (fromRate === toRate) return input;
  const ratio = fromRate / toRate;
  const length = Math.max(1, Math.round(input.length / ratio));
  const output = new Float32Array(length);
  for (let index = 0; index < length; index += 1) {
    const position = index * ratio;
    const lower = Math.floor(position);
    const fraction = position - lower;
    const first = input[lower] ?? 0;
    const second = input[lower + 1] ?? first;
    output[index] = first + (second - first) * fraction;
  }
  return output;
}

/** 浮点采样转 s16le：负半轴到 0x8000，正半轴到 0x7FFF，超出范围先夹紧。 */
export function encodePcm16(samples: Float32Array): Int16Array {
  const output = new Int16Array(samples.length);
  for (let index = 0; index < samples.length; index += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[index]));
    output[index] = clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff;
  }
  return output;
}

/** 有界帧队列：满了丢最旧的一帧，并记住累计丢弃数。 */
export class BoundedFrameQueue {
  private readonly frames: ArrayBuffer[] = [];
  private dropped = 0;

  constructor(private readonly limit: number = MAX_QUEUED_FRAMES) {}

  /** 入队一帧；返回累计丢弃帧数。 */
  push(frame: ArrayBuffer): number {
    if (this.frames.length >= this.limit) {
      this.frames.shift();
      this.dropped += 1;
    }
    this.frames.push(frame);
    return this.dropped;
  }

  /** 取出一帧；空队列返回 undefined。 */
  shift(): ArrayBuffer | undefined {
    return this.frames.shift();
  }

  get size(): number {
    return this.frames.length;
  }

  get droppedFrames(): number {
    return this.dropped;
  }

  clear(): void {
    this.frames.length = 0;
  }
}
