import React from 'react';
import { AlertCircleIcon, MicrophoneIcon } from '../../icons';
import type { ComposerPermissionPrompt, ComposerVoice } from './useComposerVoice';
import type { VoicePhase } from '../../speech/voiceProtocol';
import { VOICE_UI_ENABLED } from '../../speech/enabled';
import { VoiceWave } from './VoiceWave';
import { Banner, StateDot } from '../ui/surfaces';
import { Button } from '../ui/controls';
import { Dialog } from '../ui/overlays';

/*
 * 语音在界面上的呈现分两处：输入坞内部的语音态带，与覆盖其上的授权引导。
 *
 * 语音不是外挂于输入坞的横条，而是输入坞自身的形态变化：非待机时，坞内顶部长出
 * 一条状态带，并且同一时刻只强调一个焦点。四态各有专属标识与一个词，远看分得清，
 * 近看不必在计时、标签、声波之间做阅读理解。识别中的文字出现时直接成为那一态的焦点。
 *
 * 授权引导用应用内原生对话框而不是浏览器权限条：说清是一次性说明还是系统拦截，
 * 并在后者给出一键打开系统麦克风设置与重新检测。
 */

/** 准备期超过这个时长就说明是冷启动：给出诚实的等待预期。 */
const COLD_START_HINT_MS = 3_000;

/** 每个相位的唯一焦点：一个标识族加一个词；accent 表示正在进行、值得看。 */
interface VoiceFocus {
  label: string;
  glyph: 'wave' | 'spin' | 'dot';
  accent: boolean;
}

/** 四态可辨：准备与思考用转圈，聆听与朗读用声波，再靠词与语气分开。 */
const PHASE_FOCUS: Record<Exclude<VoicePhase, 'idle'>, VoiceFocus> = {
  arming: { label: '准备中', glyph: 'spin', accent: true },
  listening: { label: '聆听中', glyph: 'wave', accent: true },
  thinking: { label: '整理中', glyph: 'spin', accent: false },
  speaking: { label: '朗读中', glyph: 'wave', accent: true },
  interrupted: { label: '已打断', glyph: 'dot', accent: false },
};

/**
 * 输入坞内的语音态带；待机不渲染，不占高度。
 *
 * 单一焦点：聆听时让给实时转写，冷启动超时让给连接预期，其余相位就是那一态的词。
 * 计时不再与状态并列，听写的 120 秒上限极少触发，这一刻真正要说的是「正在发生什么」。
 */
export const ComposerVoiceBar: React.FC<{ voice: ComposerVoice }> = ({ voice }) => {
  if (voice.phase === 'idle') return null;
  const focus = PHASE_FOCUS[voice.phase];
  const partial = voice.partial.trim();
  const primary =
    voice.phase === 'listening' && partial.length > 0
      ? partial
      : voice.phase === 'arming' && voice.elapsedMs >= COLD_START_HINT_MS
        ? '正在连接语音提供方'
        : focus.label;
  const ink = focus.accent ? 'text-text-link' : 'text-text-tertiary';
  return (
    <div
      role="status"
      aria-live="polite"
      data-voice-status={voice.phase}
      className="flex min-w-0 items-center gap-2 px-3 pb-1 pt-2 text-meta"
    >
      <span className={`flex h-4 w-4 shrink-0 items-center justify-center ${ink}`} aria-hidden="true">
        {focus.glyph === 'wave' ? (
          <VoiceWave active={voice.phase === 'listening'} size="xs" />
        ) : focus.glyph === 'spin' ? (
          <StateDot state="ongoing" tone={focus.accent ? 'accent' : 'default'} />
        ) : (
          <span className="h-1.5 w-1.5 rounded-full bg-current" />
        )}
      </span>
      <span className={`min-w-0 flex-1 truncate ${ink} ${focus.accent ? 'font-medium' : ''}`}>
        {primary}
      </span>
      {voice.phase === 'listening' && <span className="shrink-0 text-text-tertiary">Esc 关闭</span>}
    </div>
  );
};
/** 失败结论用 Banner 表达，入口随后由控制器复位。 */
export const ComposerVoiceError: React.FC<{ message: string }> = ({ message }) => (
  <div role="alert" className="mb-2">
    <Banner tone="danger" icon={<AlertCircleIcon size={14} />} title={message} />
  </div>
);

/** 原生麦克风授权引导：一次性说明与系统拦截共用一套几何，动作不同。 */
export const ComposerVoicePermissionDialog: React.FC<{
  prompt: ComposerPermissionPrompt;
  onConfirm: () => void;
  onDismiss: () => void;
  onOpenSettings: () => void;
  onRetry: () => void;
}> = ({ prompt, onConfirm, onDismiss, onOpenSettings, onRetry }) => {
  const blocked = prompt.kind === 'system';
  return (
    <Dialog
      open
      onClose={onDismiss}
      title={prompt.title}
      size="sm"
      hint={blocked ? '系统设置里的开关改动后，回到这里重新检测即可生效。' : undefined}
      primary={{
        label: blocked ? '重新检测' : '允许使用麦克风',
        onClick: blocked ? onRetry : onConfirm,
      }}
      secondary={
        blocked && prompt.canOpenSettings
          ? { label: prompt.settingsLabel, onClick: onOpenSettings }
          : { label: '暂不使用', onClick: onDismiss }
      }
    >
      <div className="space-y-3">
        <p className="whitespace-pre-wrap break-words text-row text-text-prose">
          {prompt.description}
        </p>
        {!blocked && (
          <div className="flex items-center gap-2 rounded-sm bg-surface-sunken px-2.5 py-2">
            <MicrophoneIcon size={14} className="shrink-0 text-text-tertiary" />
            <span className="min-w-0 flex-1 text-meta text-text-tertiary">
              说话时输入框上方会显示声波与实时转写
            </span>
            <VoiceWave active={false} size="xs" className="text-text-tertiary" />
          </div>
        )}
        {blocked && prompt.canOpenSettings && (
          <Button variant="outline" size="sm" onClick={onOpenSettings}>
            {prompt.settingsLabel}
          </Button>
        )}
      </div>
    </Dialog>
  );
};

/**
 * 坞外的语音覆盖层：失败回执与授权引导。
 * 语音态带 `ComposerVoiceBar` 长在输入坞内部，由 InputDock 直接渲染，
 * 因为它是坞自身的形态变化，不是挂在坞外的另一条横条。
 */
export const ComposerVoiceLayer: React.FC<{ voice: ComposerVoice }> = ({ voice }) => {
  if (!VOICE_UI_ENABLED) return null;
  return (
    <>
      {voice.error !== null && <ComposerVoiceError message={voice.error} />}
      {voice.permission !== null && (
        <ComposerVoicePermissionDialog
          prompt={voice.permission}
          onConfirm={voice.confirmPermission}
          onDismiss={voice.dismissPermission}
          onOpenSettings={voice.openSettings}
          onRetry={voice.retryPermission}
        />
      )}
    </>
  );
};
