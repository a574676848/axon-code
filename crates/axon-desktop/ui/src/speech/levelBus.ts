/*
 * 声学电平总线：麦克风采集与朗读播放共用的实时振幅。
 *
 * 振幅每秒变化几十次，若进入 React 状态会带着整条消息列表重渲染。
 * 因此这里只做「发布 + 订阅」：需要的组件自己订阅并平滑到柱体上，
 * 其余组件一次都不重渲染。
 */

/** 订阅者：收到归一化后的振幅（0..1）。 */
type LevelListener = (level: number) => void;

const listeners = new Set<LevelListener>();

/** 当前振幅；没有发声时是 0。 */
let current = 0;

/** 把任意分贝或 RMS 结果收敛到 0..1。 */
export function normalizeLevel(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 0;
  return Math.min(1, value);
}

/** 发布一次振幅；相同值不重复广播。 */
export function publishLevel(level: number): void {
  const next = normalizeLevel(level);
  if (next === current) return;
  current = next;
  for (const listener of listeners) listener(next);
}

/** 停止发声时把总线归零。 */
export function resetLevel(): void {
  publishLevel(0);
}

/** 当前振幅快照。 */
export function currentLevel(): number {
  return current;
}

/** 订阅振幅变化。 */
export function subscribeLevel(listener: LevelListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/*
 * 播放进度通道：与振幅同一条理由，进度每秒刷新数次，
 * 进 React 状态会让整条消息列表跟着重渲染，因此同样只发布不存状态。
 */
let progress = 0;
const progressListeners = new Set<(value: number) => void>();

/** 发布一次播放进度（0..1）；相等值不重复广播。 */
export function publishProgress(value: number): void {
  const next = Math.min(1, Math.max(0, Number.isFinite(value) ? value : 0));
  if (next === progress) return;
  progress = next;
  for (const listener of progressListeners) listener(next);
}

/** 归零并广播进度。 */
export function resetProgress(): void {
  publishProgress(0);
}

/** 订阅播放进度。 */
export function subscribeProgress(listener: (value: number) => void): () => void {
  progressListeners.add(listener);
  return () => {
    progressListeners.delete(listener);
  };
}

/** 由 s16le 小端字节计算 RMS 振幅，供流式播放与采集共用。 */
export function levelOfPcm16(bytes: Uint8Array): number {
  if (bytes.length < 2) return 0;
  const samples = Math.floor(bytes.length / 2);
  let sum = 0;
  for (let index = 0; index < samples; index += 1) {
    const low = bytes[index * 2];
    const high = bytes[index * 2 + 1];
    const sample = ((high << 8) | low) << 16 >> 16;
    const normalized = sample / 32768;
    sum += normalized * normalized;
  }
  const rms = Math.sqrt(sum / samples);
  // 说话时的 RMS 大致落在 0.02 到 0.2，放大后柱体才有明显起伏。
  return normalizeLevel(rms * 4.2);
}

/** 由浮点采样计算 RMS 振幅，供麦克风采集使用。 */
export function levelOfSamples(samples: Float32Array): number {
  if (samples.length === 0) return 0;
  let sum = 0;
  for (let index = 0; index < samples.length; index += 1) {
    const value = samples[index];
    sum += value * value;
  }
  return normalizeLevel(Math.sqrt(sum / samples.length) * 3.6);
}
