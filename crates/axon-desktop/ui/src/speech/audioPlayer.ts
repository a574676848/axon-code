import type { PlayerEvents, VoiceAudioPayload, VoicePlayerHandle } from './controller';
import { publishLevel, publishProgress, resetLevel, resetProgress } from './levelBus';

/*
 * 浏览器播放器：把 base64 音频做成 Blob 地址交给 Audio。
 *
 * 合成结果是真实的 wav 字节，不能按文本解码，否则二进制里的高位会被换掉。
 * 停止与收口都必须释放地址：不释放的话解码后的音频会一直占着内存，
 * 反复朗读几百条消息就是几百份音频留在那里。
 *
 * 同时接一条分析链：朗读按钮上的声波与进度线由实测振幅和时间推进驱动，
 * 因此它反映的是这一刻真的在响，而不是一段按固定节奏播放的装饰动画。
 */

/** base64 还原成原始字节。 */
function decodeBase64(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

/** 分析窗口：512 样本覆盖约 11 ms，足以跟上说话的起伏。 */
const ANALYSER_FFT = 512;

export function createAudioPlayer(
  payload: VoiceAudioPayload,
  events: PlayerEvents,
): VoicePlayerHandle {
  const blob = new Blob([decodeBase64(payload.audioBase64) as unknown as BlobPart], {
    type: payload.mimeType,
  });
  const url = URL.createObjectURL(blob);
  const audio = new Audio(url);
  let released = false;
  let frame = 0;
  let context: AudioContext | null = null;
  let analyser: AnalyserNode | null = null;

  /** 采样实时波形，把振幅与播放进度推给订阅者。 */
  const sample = (): void => {
    if (released) return;
    frame = window.requestAnimationFrame(sample);
    if (analyser === null) return;
    const data = new Uint8Array(analyser.fftSize);
    analyser.getByteTimeDomainData(data as Uint8Array<ArrayBuffer>);
    let sum = 0;
    for (let index = 0; index < data.length; index += 1) {
      const deviation = (data[index] - 128) / 128;
      sum += deviation * deviation;
    }
    publishLevel(Math.sqrt(sum / data.length) * 3.4);
    const duration = audio.duration;
    if (Number.isFinite(duration) && duration > 0) {
      publishProgress(audio.currentTime / duration);
    }
  };

  /** 挂上分析链；WebAudio 不可用时退回纯播放，不因装饰失败而拦下朗读。 */
  const attachAnalyser = (): void => {
    if (typeof AudioContext === 'undefined') return;
    try {
      context = new AudioContext();
      const source = context.createMediaElementSource(audio);
      analyser = context.createAnalyser();
      analyser.fftSize = ANALYSER_FFT;
      source.connect(analyser);
      analyser.connect(context.destination);
      void context.resume().catch(() => undefined);
      frame = window.requestAnimationFrame(sample);
    } catch {
      context = null;
      analyser = null;
    }
  };

  const release = (): void => {
    if (released) return;
    released = true;
    audio.onended = null;
    audio.onerror = null;
    if (frame !== 0) {
      window.cancelAnimationFrame(frame);
      frame = 0;
    }
    analyser?.disconnect();
    analyser = null;
    if (context !== null) {
      void context.close().catch(() => undefined);
      context = null;
    }
    resetLevel();
    resetProgress();
    URL.revokeObjectURL(url);
  };

  audio.onended = () => {
    release();
    events.onEnded();
  };
  audio.onerror = () => {
    release();
    events.onFailed(new Error('音频播放失败'));
  };

  return {
    play: () => {
      attachAnalyser();
      return audio.play();
    },
    stop: () => {
      audio.pause();
      release();
    },
  };
}
