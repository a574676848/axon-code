import React from 'react';
import {
  describeAccessBlock,
  hasMicrophoneConsent,
  isBlockedBySystem,
  openMicrophoneSettings,
  readMicrophoneAccess,
  rememberMicrophoneConsent,
  type MicrophoneAccess,
  type MicrophoneAccessState,
} from '../../speech/voicePermission';

/*
 * 麦克风授权的应用内引导。
 *
 * 宿主层已经对本地受信任源静默放行 WebView 权限，所以要处理的只有两件事：
 * 首次使用时给一次说明，以及操作系统隐私层拒绝时把用户送到系统设置。
 * 两种情形共用同一个对话框，动作不同：前者「允许使用麦克风」，后者「重新检测」。
 */

/** 定稿落点；说明文案与入口一致。 */
export type VoiceMode = 'dictation' | 'conversation';

/** 麦克风授权弹窗的形态。 */
export interface ComposerPermissionPrompt {
  kind: 'first-use' | 'system';
  /** 系统的真实结论，只用于文案与是否给「打开设置」。 */
  state: MicrophoneAccessState;
  title: string;
  description: string;
  settingsLabel: string;
  canOpenSettings: boolean;
}

export interface VoicePermissionOptions {
  /** 定稿落点；只影响首次说明的措辞。 */
  mode: VoiceMode;
  /** 用户在首次说明里确认后继续开始录音。 */
  onConfirmed: () => void;
  /** 系统设置打不开之类的失败结论。 */
  onError: (message: string) => void;
}

export interface VoicePermission {
  /** 授权引导；为空时不显示。 */
  prompt: ComposerPermissionPrompt | null;
  /** 授权是否已经放行；需要引导时返回 false 并弹出对应说明。 */
  ensureAccess: () => Promise<boolean>;
  /** 用户确认首次说明。 */
  confirm: () => void;
  /** 关掉引导。 */
  dismiss: () => void;
  /** 打开系统麦克风隐私设置。 */
  openSettings: () => void;
  /** 重新检测系统权限。 */
  retry: () => void;
}

/** 系统拒绝时给出可执行的引导，而不是一句「权限被拒绝」。 */
function systemPrompt(access: MicrophoneAccess): ComposerPermissionPrompt {
  return {
    kind: 'system',
    state: access.state,
    title: '系统麦克风权限未放行',
    description: `${describeAccessBlock(access)}。请到系统设置里允许 Axon 使用麦克风，然后回到这里重新检测。`,
    settingsLabel: access.settingsLabel,
    canOpenSettings: access.canOpenSettings,
  };
}

/** 首次使用的一次性说明：本机处理，不录音留存。 */
function firstUsePrompt(mode: VoiceMode): ComposerPermissionPrompt {
  return {
    kind: 'first-use',
    state: 'unknown',
    title: '允许 Axon 使用麦克风',
    description:
      mode === 'dictation'
        ? '语音只用于本次输入：音频在本机语音运行时里识别，识别结果写进输入框，由你决定是否发送；不录音留存、不上传云端。'
        : '语音对话会边听边说：音频在本机语音运行时里识别，回复同样由本机合成朗读；不录音留存、不上传云端。',
    settingsLabel: '打开系统麦克风设置',
    canOpenSettings: false,
  };
}

/** 把未知异常翻成中文结论。 */
function describeFailure(failure: unknown): string {
  if (failure instanceof Error && failure.message.length > 0) return failure.message;
  if (typeof failure === 'string' && failure.length > 0) return failure;
  return '无法打开系统设置，请手动到系统隐私设置里允许 Axon 使用麦克风';
}

export function useVoicePermission({
  mode,
  onConfirmed,
  onError,
}: VoicePermissionOptions): VoicePermission {
  const [prompt, setPrompt] = React.useState<ComposerPermissionPrompt | null>(null);
  const confirmedRef = React.useRef(onConfirmed);
  confirmedRef.current = onConfirmed;

  const ensureAccess = React.useCallback(async (): Promise<boolean> => {
    if (!hasMicrophoneConsent()) {
      setPrompt(firstUsePrompt(mode));
      return false;
    }
    const access = await readMicrophoneAccess().catch(() => null);
    if (access !== null && isBlockedBySystem(access)) {
      setPrompt(systemPrompt(access));
      return false;
    }
    return true;
  }, [mode]);

  const confirm = React.useCallback(() => {
    rememberMicrophoneConsent();
    setPrompt(null);
    confirmedRef.current();
  }, []);

  const dismiss = React.useCallback(() => setPrompt(null), []);

  const openSettings = React.useCallback(() => {
    void openMicrophoneSettings().catch((failure: unknown) => {
      onError(describeFailure(failure));
    });
  }, [onError]);

  const retry = React.useCallback(() => {
    void readMicrophoneAccess()
      .then((access) => {
        if (isBlockedBySystem(access)) {
          setPrompt(systemPrompt(access));
          return;
        }
        setPrompt(null);
        confirmedRef.current();
      })
      .catch((failure: unknown) => onError(describeFailure(failure)));
  }, [onError]);

  return { prompt, ensureAccess, confirm, dismiss, openSettings, retry };
}
