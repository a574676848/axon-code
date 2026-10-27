import React from 'react';
import { AlertCircleIcon } from '../../icons';
import { VOICE_PHASE_TEXT, type VoicePhase } from '../../speech/voiceProtocol';
import { Banner, StateDot } from '../ui/surfaces';

/*
 * 语音的两层呈现：输入坞上方的状态行与贴在它上沿的中间结果浮层。
 *
 * 状态行沿用工作状态行的几何（28px 下沉面药丸），只是文案来自 PROTOCOL 2.3 的
 * 六态；中间结果用固定 20px 的浮层，不参与布局，因此 partial 频繁覆盖时输入坞
 * 不会跳动。定稿不在这里显示，它直接写进正文。
 */

/** 六态状态行；待机不占行。 */
export const ComposerVoiceStatus: React.FC<{
  phase: VoicePhase;
  droppedFrames: number;
}> = ({ phase, droppedFrames }) => {
  if (phase === 'idle') return null;
  return (
    <div
      role="status"
      aria-live="polite"
      className="popover-up-in relative mb-2 px-2.5 h-7 flex items-center gap-2 overflow-hidden rounded-sm bg-surface-sunken/85 hairline border-border"
    >
      <StateDot state={phase === 'interrupted' ? 'warning' : 'ongoing'} />
      <span className="min-w-0 truncate text-meta font-medium text-text-primary">
        {VOICE_PHASE_TEXT[phase]}
      </span>
      {droppedFrames > 0 && (
        <span className="shrink-0 text-meta tabular-nums text-text-tertiary">
          已丢弃 {droppedFrames} 帧
        </span>
      )}
    </div>
  );
};

/** 中间识别结果：固定 20px 浮层，不改变输入坞版面。 */
export const ComposerVoiceOverlay: React.FC<{ text: string }> = ({ text }) => {
  if (text.length === 0) return null;
  return (
    <div
      aria-live="polite"
      className="pointer-events-none absolute inset-x-0 bottom-full mb-1 flex justify-center px-4"
    >
      <div className="flex h-5 w-full max-w-chat items-center rounded-sm bg-surface/95 px-2.5 hairline border-border-hairline shadow-panel">
        <span className="min-w-0 truncate text-meta text-text-prose">{text}</span>
      </div>
    </div>
  );
};

/** 失败结论用 Banner 表达，入口随后由控制器复位。 */
export const ComposerVoiceError: React.FC<{ message: string }> = ({ message }) => (
  <div role="alert" className="mb-2">
    <Banner tone="danger" icon={<AlertCircleIcon size={14} />} title={message} />
  </div>
);
