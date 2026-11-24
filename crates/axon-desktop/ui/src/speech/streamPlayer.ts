import { levelOfPcm16, publishLevel, resetLevel } from './levelBus';

/*
 * 下行朗读音频的实时播放器。
 *
 * 网关按 `0x02` 帧把 16 kHz 单声道 s16le 分块推下来，每块约 200 ms。
 * 若每块新建一个 `Audio` 元素，块之间必然出现缝隙与爆音，因此这里把
 * 采样解码成 AudioBuffer，按 `AudioContext.currentTime` 连续排程，
 * 同时用块内 RMS 驱动声波、播完回调通知宿主发送 `playbackDrained`。
 */

/** 协议的采样率。 */
const STREAM_SAMPLE_RATE = 16_000;

/** 首个块至少提前这么久排程，避免「现在才开始」导致的爆音。 */
const START_LEAD_SECONDS = 0.06;

/** 播放器回调。 */
export interface StreamPlayerEvents {
  /** 收口：所有已排程的块都播完了。 */
  onDrained: () => void;
  /** 播放不可用或抛出异常。 */
  onError: (message: string) => void;
}

/** 一个可按块推送的播放器。 */
export interface VoiceStreamPlayer {
  /** 推入一块 base64 编码的 s16le PCM。 */
  push(pcmBase64: string): void;
  /** 立即停播并清空排程；`drain` 为真时随后仍会回调收口。 */
  stop(options?: { drain?: boolean }): void;
  /** 释放音频上下文。 */
  destroy(): void;
}

/** base64 还原成字节。 */
function decodeBase64(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

/** 小端 s16 转浮点采样。 */
function toFloatSamples(bytes: Uint8Array): Float32Array {
  const samples = Math.floor(bytes.length / 2);
  const output = new Float32Array(samples);
  for (let index = 0; index < samples; index += 1) {
    const low = bytes[index * 2];
    const high = bytes[index * 2 + 1];
    const value = ((high << 8) | low) << 16 >> 16;
    output[index] = value / 32768;
  }
  return output;
}

/** 创建流式播放器；`AudioContext` 在首次推块时才建立。 */
export function createVoiceStreamPlayer(events: StreamPlayerEvents): VoiceStreamPlayer {
  let context: AudioContext | null = null;
  let nextAt = 0;
  let sources = new Set<AudioBufferSourceNode>();
  let stopped = false;
  let drained = false;

  const ensureContext = (): AudioContext | null => {
    if (context !== null) return context;
    if (typeof AudioContext === 'undefined') {
      events.onError('当前窗口不支持音频播放');
      return null;
    }
    try {
      context = new AudioContext();
    } catch (failure) {
      events.onError(failure instanceof Error ? failure.message : '音频播放不可用');
      return null;
    }
    return context;
  };

  const finish = (): void => {
    if (drained) return;
    drained = true;
    resetLevel();
    events.onDrained();
  };

  /** 没有在途块时收口；有在途块时交给最后一块的 onended。 */
  const settle = (): void => {
    if (stopped) return;
    if (sources.size === 0) finish();
  };

  const push = (pcmBase64: string): void => {
    if (stopped || pcmBase64.length === 0) return;
    const ctx = ensureContext();
    if (ctx === null) return;
    const bytes = decodeBase64(pcmBase64);
    const samples = toFloatSamples(bytes);
    if (samples.length === 0) return;
    if (ctx.state === 'suspended') void ctx.resume().catch(() => undefined);

    const buffer = ctx.createBuffer(1, samples.length, STREAM_SAMPLE_RATE);
    buffer.copyToChannel(samples as Float32Array<ArrayBuffer>, 0);
    const source = ctx.createBufferSource();
    source.buffer = buffer;
    source.connect(ctx.destination);

    const now = ctx.currentTime;
    nextAt = Math.max(nextAt, now + START_LEAD_SECONDS);
    const playAt = nextAt;
    nextAt += buffer.duration;
    sources.add(source);
    source.onended = () => {
      sources.delete(source);
      if (sources.size === 0) settle();
    };
    try {
      source.start(playAt);
    } catch (failure) {
      sources.delete(source);
      events.onError(failure instanceof Error ? failure.message : '音频排程失败');
      return;
    }
    // 这一块的开头就是此刻的响度：柱体不会等整块播完才动。
    publishLevel(levelOfPcm16(bytes));
    // 峰值之后自然回落，避免在块与块之间整段贴底。
    window.setTimeout(() => {
      if (!stopped && sources.size > 0) publishLevel(0.18);
    }, 110);
  };

  const stop = (options: { drain?: boolean } = {}): void => {
    const shouldDrain = options.drain === true;
    stopped = true;
    for (const source of sources) {
      source.onended = null;
      try {
        source.stop();
      } catch {
        // 已经自然结束的块再停一次会抛，忽略即可。
      }
    }
    sources = new Set();
    nextAt = 0;
    resetLevel();
    if (shouldDrain) finish();
  };

  const destroy = (): void => {
    stop();
    if (context !== null) {
      void context.close().catch(() => undefined);
      context = null;
    }
  };

  return { push, stop, destroy };
}
