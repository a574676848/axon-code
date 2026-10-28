import { invoke } from '@tauri-apps/api/core';
import { levelOfSamples, publishLevel, resetLevel } from './levelBus';
import {
  BoundedFrameQueue,
  encodePcm16,
  MAX_QUEUED_FRAMES,
  resampleLinear,
  VOICE_FRAME_SAMPLES,
  VOICE_SAMPLE_RATE,
} from './pcm';

/*
 * 麦克风采集：16 kHz 单声道，页面内合帧到 100 ms，经 Tauri raw body 上行。
 *
 * 音频帧不经过 JSON：`voice_audio` 的第二个参数是 ArrayBuffer，由宿主按
 * `InvokeBody::Raw` 取出并补上协议帧头。这里只管采集、合帧与有界队列；
 * 连接由宿主持有，采集器不知道 WebSocket 的存在。
 *
 * 队列有界 8 帧，满时丢最旧并计数：弱网时最新的话比最早的话更重要。
 */

/** 采集器回调。 */
export interface VoiceCaptureEvents {
  /** 上行失败或采集不可用。 */
  onError: (message: string) => void;
  /** 累计丢弃帧数变化。 */
  onDropped?: (total: number) => void;
  /** 实时音量包络（0..1），供输入坞声波使用。 */
  onLevel?: (level: number) => void;
}

/** 采集参数。 */
export interface VoiceCaptureOptions {
  /** 指定输入设备；空串或省略表示跟随系统默认。明确指定的设备不会回退到别的输入。 */
  deviceId?: string;
}

/** 一个可启停的采集器。 */
export interface VoiceCapture {
  /** 申请麦克风并开始合帧；失败时抛出中文原因。 */
  start(): Promise<void>;
  /** 停止采集并释放音轨与音频上下文。 */
  stop(): void;
  /** 放行或暂停上行；暂停期间的帧仍会进入有界队列。 */
  setForwarding(enabled: boolean): void;
  /** 本次采集累计丢弃的帧数。 */
  droppedFrames(): number;
}

/** 脚本处理器的缓冲区大小；2048 在 48 kHz 下约 42 ms，足够平滑。 */
const PROCESSOR_BUFFER = 2048;

/** 电平上报间隔：与视觉刷新率对齐，避免每个处理块都触发一次。 */
const LEVEL_INTERVAL_MS = 90;

function describeFailure(failure: unknown): string {
  if (failure instanceof DOMException) {
    switch (failure.name) {
      case 'NotAllowedError':
        return '麦克风权限被拒绝，请在系统隐私设置中允许 Axon 使用麦克风';
      case 'NotFoundError':
        return '没有找到可用的麦克风';
      case 'NotReadableError':
        return '麦克风被其他应用占用，暂时无法采集';
      case 'SecurityError':
        return '当前环境不允许使用麦克风';
      default:
        break;
    }
  }
  if (failure instanceof Error && failure.message.length > 0) return failure.message;
  if (typeof failure === 'string' && failure.length > 0) return failure;
  return '麦克风采集失败';
}

/** 创建一个采集器；同一实例可以反复 start 与 stop。 */
export function createVoiceCapture(
  events: VoiceCaptureEvents,
  options: VoiceCaptureOptions = {},
): VoiceCapture {
  let queue = new BoundedFrameQueue(MAX_QUEUED_FRAMES);
  let stream: MediaStream | null = null;
  let context: AudioContext | null = null;
  let source: MediaStreamAudioSourceNode | null = null;
  let processor: ScriptProcessorNode | null = null;
  let forwarding = false;
  let sending = false;
  let stopped = true;
  let reportedDropped = 0;
  /** 尚未凑满一帧的采样点。 */
  let pending: number[] = [];
  /** 上一次上报电平的时刻。 */
  let lastLevelAt = 0;

  const reportDropped = (): void => {
    if (queue.droppedFrames === reportedDropped) return;
    reportedDropped = queue.droppedFrames;
    events.onDropped?.(reportedDropped);
  };

  const drain = async (): Promise<void> => {
    if (sending || !forwarding || stopped) return;
    sending = true;
    try {
      while (forwarding && !stopped) {
        const frame = queue.shift();
        if (frame === undefined) break;
        try {
          await invoke<void>('voice_audio', frame, {
            headers: { 'Content-Type': 'application/octet-stream' },
          });
        } catch (failure) {
          forwarding = false;
          events.onError(describeFailure(failure));
          break;
        }
      }
    } finally {
      sending = false;
    }
    if (forwarding && !stopped && queue.size > 0) void drain();
  };

  const enqueue = (frame: ArrayBuffer): void => {
    if (stopped) return;
    queue.push(frame);
    reportDropped();
    if (forwarding) void drain();
  };

  const flushFrames = (): void => {
    while (pending.length >= VOICE_FRAME_SAMPLES) {
      const chunk = pending.slice(0, VOICE_FRAME_SAMPLES);
      pending = pending.slice(VOICE_FRAME_SAMPLES);
      const encoded = encodePcm16(Float32Array.from(chunk));
      enqueue(encoded.buffer as ArrayBuffer);
    }
  };

  const release = (): void => {
    if (processor !== null) {
      processor.onaudioprocess = null;
      processor.disconnect();
      processor = null;
    }
    if (source !== null) {
      source.disconnect();
      source = null;
    }
    if (context !== null) {
      void context.close().catch(() => undefined);
      context = null;
    }
    if (stream !== null) {
      for (const track of stream.getTracks()) track.stop();
      stream = null;
    }
    pending = [];
    lastLevelAt = 0;
    resetLevel();
  };

  const start = async (): Promise<void> => {
    if (!stopped) return;
    if (typeof navigator === 'undefined' || navigator.mediaDevices === undefined) {
      throw new Error('当前环境不支持麦克风采集');
    }
    stopped = false;
    forwarding = false;
    pending = [];
    queue = new BoundedFrameQueue(MAX_QUEUED_FRAMES);
    reportedDropped = 0;
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          sampleRate: VOICE_SAMPLE_RATE,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
          ...(options.deviceId !== undefined && options.deviceId.length > 0
            ? { deviceId: { exact: options.deviceId } }
            : {}),
        },
      });
      context = new AudioContext({ sampleRate: VOICE_SAMPLE_RATE });
      if (context.state === 'suspended') {
        await context.resume().catch(() => undefined);
      }
      source = context.createMediaStreamSource(stream);
      processor = context.createScriptProcessor(PROCESSOR_BUFFER, 1, 1);
      processor.onaudioprocess = (event) => {
        if (stopped) return;
        const input = event.inputBuffer.getChannelData(0);
        const samples =
          context !== null && context.sampleRate === VOICE_SAMPLE_RATE
            ? input
            : resampleLinear(input, context?.sampleRate ?? VOICE_SAMPLE_RATE, VOICE_SAMPLE_RATE);
        for (let index = 0; index < samples.length; index += 1) {
          pending.push(samples[index]);
        }
        const now = Date.now();
        if (now - lastLevelAt >= LEVEL_INTERVAL_MS) {
          lastLevelAt = now;
          const level = levelOfSamples(samples);
          publishLevel(level);
          events.onLevel?.(level);
        }
        flushFrames();
      };
      source.connect(processor);
      processor.connect(context.destination);
    } catch (failure) {
      release();
      stopped = true;
      throw new Error(describeFailure(failure));
    }
  };

  const stop = (): void => {
    stopped = true;
    forwarding = false;
    resetLevel();
    release();
  };

  const setForwarding = (enabled: boolean): void => {
    forwarding = enabled;
    if (enabled) void drain();
  };

  return {
    start,
    stop,
    setForwarding,
    droppedFrames: () => queue.droppedFrames,
  };
}
