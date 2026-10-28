import React from 'react';
import { AlertCircleIcon, MicrophoneIcon } from '../../icons';
import type { ComposerPermissionPrompt, ComposerVoice } from './useComposerVoice';
import type { VoicePhase } from '../../speech/voiceProtocol';
import { VoiceWave } from './VoiceWave';
import { Banner } from '../ui/surfaces';
import { Button } from '../ui/controls';
import { Dialog } from '../ui/overlays';

/*
 * 语音的三层呈现：输入坞上方的录音条、贴在上沿的中间结果浮层，与原生授权引导。
 *
 * 录音条借用成熟语音输入产品的形态：一条 44px 的下沉面横条，左侧是实测振幅驱动的
 * 声波与计时，右侧只放两个动作，「停止并识别」与「取消」。停止会把识别结果写进
 * 输入坞，取消则整句丢弃，草稿保持原样；两种结果都不直接发送。
 *
 * 中间结果用固定 20px 的浮层，不参与布局，partial 频繁覆盖时输入坞不会跳动。
 * 授权引导用应用内原生对话框而不是浏览器权限条：说清是一次性说明还是系统拦截，
 * 并在后者给出一键打开系统麦克风设置与重新检测。
 */

/** 录音条上的动作词：一句就够，不重复「聆听 / 正在录音」。 */
const BAR_LABEL: Record<Exclude<VoicePhase, 'idle'>, string> = {
  arming: '准备中',
  listening: '录音中',
  thinking: '整理中',
  speaking: '朗读中',
  interrupted: '已打断',
};

/** 录音条上的补充说明：只说下一步能做什么。 */
const BAR_HINT: Record<Exclude<VoicePhase, 'idle'>, string> = {
  arming: '',
  listening: '',
  thinking: '识别结果会写进输入框',
  speaking: '',
  interrupted: '',
};

/** 准备期超过这个时长就说明是冷启动：给出诚实的等待预期。 */
const COLD_START_HINT_MS = 3_000;

/** 录音条上的时间读数：分:秒，秒恒两位。 */
function formatClock(durationMs: number): string {
  const total = Math.max(0, Math.floor(durationMs / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, '0')}`;
}

/**
 * 语音录音条；待机不占高度。
 *
 * 只讲用户能感知的事实：在做什么、还能做什么。丢帧这类内部指标不进这一条。
 */
export const ComposerVoiceBar: React.FC<{ voice: ComposerVoice }> = ({ voice }) => {
  if (voice.phase === 'idle') return null;
  const recording = voice.phase === 'listening';
  const partial = voice.partial.trim();
  const hint =
    voice.phase === 'arming' && voice.elapsedMs >= COLD_START_HINT_MS
      ? '首次连接要加载本地语音模型'
      : partial.length > 0
        ? partial
        : BAR_HINT[voice.phase];
  return (
    <div
      role="status"
      aria-live="polite"
      data-voice-status={voice.phase}
      className="popover-up-in relative mb-1.5 flex h-10 items-center gap-2 rounded-md bg-surface px-2.5 shadow-panel hairline border-border"
    >
      {/* 录音中：一枚品牌色小点加实测振幅声波，说明这一条正在收声。 */}
      {recording ? (
        <span className="flex shrink-0 items-center gap-1.5 text-text-link">
          <span className="h-1.5 w-1.5 rounded-full bg-current" aria-hidden="true" />
          <VoiceWave active size="xs" />
        </span>
      ) : (
        <span className="h-3 w-3 shrink-0 animate-spin rounded-full border border-text-tertiary border-t-transparent" />
      )}
      <span className="shrink-0 text-meta font-medium text-text-primary">
        {BAR_LABEL[voice.phase]}
      </span>
      <span className="shrink-0 text-meta tabular-nums text-text-tertiary">
        {formatClock(voice.elapsedMs)}
      </span>
      {/*
        识别中的文字就长在这一条上：它属于「这次录音」的一部分，
        不该另起一块浮层，也不该等到停止后才出现。
      */}
      <span
        className={`min-w-0 flex-1 truncate text-meta ${
          partial.length > 0 ? 'text-text-prose' : 'text-text-tertiary'
        }`}
      >
        {hint}
      </span>
      {(recording || voice.phase === 'thinking' || voice.phase === 'arming') && (
        <span className="shrink-0 text-meta text-text-tertiary">Esc 关闭</span>
      )}
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

/** 输入坞里的语音组合：失败回执、录音条与授权引导。 */
export const ComposerVoiceLayer: React.FC<{ voice: ComposerVoice }> = ({ voice }) => (
  <>
    {voice.error !== null && <ComposerVoiceError message={voice.error} />}
    <ComposerVoiceBar voice={voice} />
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
