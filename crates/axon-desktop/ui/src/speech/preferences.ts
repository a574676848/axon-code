import { VOICE_STORAGE_KEYS } from './keys';

/*
 * 语音播报的偏好存取。
 *
 * 桌面端现有的偏好分两处：主题与通知落在宿主状态里（走 IPC），
 * 侧栏布局落在 localStorage。语音播报没有对应的宿主字段，
 * 因此沿用本地存储这一路，并把键名集中到 keys.ts。
 *
 * 两层开关：全局开关决定新回复默认是否朗读；会话开关按「工作区 + 会话」单独存。
 * 会话没有显式设置时继承全局值，点过就只作用于这一条会话。
 * 音色预设另占一个键：空串表示跟随人格，合成时由后端按人格融合选音色。
 */

/** 存取后端：测试注入内存实现，运行时是 localStorage。 */
export interface PreferenceStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** 语音偏好：读、写与订阅。 */
export interface VoicePreferences {
  /** 全局开关；没有存过时为 false。 */
  globalAutoRead(): boolean;
  /** 会话的显式设置；没存过时为 null，表示继承全局。 */
  laneAutoRead(laneKey: string): boolean | null;
  /** 这一条会话此刻是否自动朗读；laneKey 为空时取全局值。 */
  effectiveAutoRead(laneKey: string | null): boolean;
  /** 音色预设 id；没存过或存的是空串时为 ''，表示跟随人格。 */
  presetId(): string;
  setGlobalAutoRead(next: boolean): void;
  setLaneAutoRead(laneKey: string, next: boolean): void;
  /** 写入音色预设；空串表示跟随人格。 */
  setPresetId(next: string): void;
  subscribe(listener: () => void): () => void;
}

/** 把存下来的文本归一成布尔：只有明确的 true 才算打开。 */
function parseFlag(raw: string | null): boolean | null {
  if (raw === 'true') return true;
  if (raw === 'false') return false;
  return null;
}

/** 创建一个偏好实例。storage 为 null 时全部取默认值，只在内存里生效。 */
export function createVoicePreferences(storage: PreferenceStorage | null): VoicePreferences {
  const listeners = new Set<() => void>();

  const read = (key: string): string | null => {
    if (storage === null) return null;
    try {
      return storage.getItem(key);
    } catch {
      return null;
    }
  };

  const write = (key: string, value: string): void => {
    if (storage === null) return;
    try {
      storage.setItem(key, value);
    } catch {
      // 写失败只影响下次启动的记住状态，当前这一次照常生效。
    }
  };

  const notify = (): void => {
    for (const listener of listeners) listener();
  };

  const globalAutoRead = (): boolean => parseFlag(read(VOICE_STORAGE_KEYS.global)) === true;

  const laneAutoRead = (laneKey: string): boolean | null =>
    parseFlag(read(VOICE_STORAGE_KEYS.lane(laneKey)));

  const presetId = (): string => read(VOICE_STORAGE_KEYS.preset) ?? '';

  return {
    globalAutoRead,
    laneAutoRead,
    presetId,
    effectiveAutoRead: (laneKey) =>
      laneKey === null ? globalAutoRead() : (laneAutoRead(laneKey) ?? globalAutoRead()),
    setGlobalAutoRead: (next) => {
      write(VOICE_STORAGE_KEYS.global, next ? 'true' : 'false');
      notify();
    },
    setLaneAutoRead: (laneKey, next) => {
      write(VOICE_STORAGE_KEYS.lane(laneKey), next ? 'true' : 'false');
      notify();
    },
    setPresetId: (next) => {
      write(VOICE_STORAGE_KEYS.preset, next);
      notify();
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

/**
 * 浏览器存储：调用时才去取 localStorage。
 *
 * 不在模块加载时判定一次：那会把「没有 window」的结论钉死在整个进程里，
 * 而真正需要的只是每次读写时确认宿主还在。存取本身失败由上面两层兜住。
 */
const browserStorage: PreferenceStorage = {
  getItem: (key) => (typeof window === 'undefined' ? null : window.localStorage.getItem(key)),
  setItem: (key, value) => {
    if (typeof window === 'undefined') return;
    window.localStorage.setItem(key, value);
  },
};

/** 全应用共用的一份语音偏好。 */
export const voicePreferences = createVoicePreferences(browserStorage);
