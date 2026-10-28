import { call } from '../backend';
import { voiceRuntimeStatus } from '../ipcService';

/*
 * 原生麦克风授权的宿主通道。
 *
 * 桌面应用不使用浏览器式权限弹窗：WebView 层由宿主对本地受信任源静默放行，
 * 真正决定能否采集的是操作系统（Windows 隐私设置、macOS 隐私与安全性）。
 * 因此这一层只做两件事：查询系统结论、以及把用户送到系统的麦克风设置页。
 */

/** 系统麦克风隐私结论。 */
export type MicrophoneAccessState =
  | 'granted'
  | 'system_denied'
  | 'desktop_denied'
  | 'unknown';

/** 宿主回执。 */
export interface MicrophoneAccess {
  state: MicrophoneAccessState;
  canOpenSettings: boolean;
  settingsLabel: string;
}

/** 首次授权说明的落盘键：确认过一次之后不再重复打断。 */
const MICROPHONE_CONSENT_KEY = 'axon.voice.mic-consent';

/** 查询系统麦克风隐私状态。 */
export async function readMicrophoneAccess(): Promise<MicrophoneAccess> {
  const raw = (await call<Record<string, unknown>>('voice_microphone_status')) ?? {};
  const state = typeof raw.state === 'string' ? raw.state : 'unknown';
  return {
    state:
      state === 'granted' || state === 'system_denied' || state === 'desktop_denied'
        ? state
        : 'unknown',
    canOpenSettings: raw.canOpenSettings === true,
    settingsLabel:
      typeof raw.settingsLabel === 'string' && raw.settingsLabel.length > 0
        ? raw.settingsLabel
        : '打开系统麦克风设置',
  };
}

/** 打开操作系统的麦克风隐私或音频输入设置。 */
export async function openMicrophoneSettings(): Promise<void> {
  await call('voice_open_microphone_settings');
}

/** 用户是否已经确认过「允许使用麦克风」的说明。 */
export function hasMicrophoneConsent(): boolean {
  if (typeof window === 'undefined') return false;
  try {
    return window.localStorage.getItem(MICROPHONE_CONSENT_KEY) === 'true';
  } catch {
    return false;
  }
}

/** 记下用户已经确认过说明。 */
export function rememberMicrophoneConsent(): void {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(MICROPHONE_CONSENT_KEY, 'true');
  } catch {
    // 写不进去只影响下次是否再问一遍，不影响本次授权。
  }
}

/** 系统层面的拒绝是「需要用户去系统里改」，与设备占用之类的临时失败要分开处理。 */
export function isBlockedBySystem(access: MicrophoneAccess): boolean {
  return access.state === 'system_denied' || access.state === 'desktop_denied';
}

/** 把系统结论翻成一句可执行的中文。 */
export function describeAccessBlock(access: MicrophoneAccess): string {
  if (access.state === 'system_denied') {
    return '系统麦克风总开关已关闭，Axon 无法采集语音';
  }
  if (access.state === 'desktop_denied') {
    return '系统禁止桌面应用访问麦克风，Axon 无法采集语音';
  }
  return '系统麦克风权限尚未放行，Axon 无法采集语音';
}

/** 本地语音运行时的就绪结论。 */
export interface VoiceRuntime {
  /** 运行时与模型都齐备。 */
  available: boolean;
  /** 不可用时的稳定原因码；可用时为 null。 */
  degraded: string | null;
}

/**
 * 读取本地语音运行时就绪状态。
 *
 * 与建连时的 `ready` 共用网关同一判据，所以在点麦克风之前就能知道「装了没装」，
 * 不必先等一次冷启动失败。网关读不到时返回 null，由调用方决定继续尝试还是提示。
 */
export async function readVoiceRuntime(): Promise<VoiceRuntime | null> {
  try {
    const raw = (await voiceRuntimeStatus()) ?? {};
    return {
      available: raw.runtimeAvailable === true,
      degraded: typeof raw.degraded === 'string' ? raw.degraded : null,
    };
  } catch {
    return null;
  }
}

/** 运行时不可用时的可执行结论。 */
export const VOICE_RUNTIME_MISSING_TEXT =
  '本地语音运行时不可用：重新运行 Axon 安装脚本补齐语音依赖后重试';
