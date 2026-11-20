import React from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'motion/react';
import { ArrowUpIcon, MicrophoneIcon, PlusIcon, StopIcon } from '../../icons';
import { VOICE_UI_ENABLED } from '../../speech/enabled';
import { ComposerPicker } from './ComposerPicker';

/*
 * 输入坞底部操作行：附件入口、键盘提示、模型与推理强度选择器、发送/中断主按钮。
 *
 * 发送、忙碌与停止三态通过阻尼弹簧平滑翻转，并在减弱动效偏好下自动退化为瞬时切换。
 */

const ICON_SPRING = { type: 'spring' as const, stiffness: 520, damping: 32 };

export interface ComposerActionBarProps {
  compact: boolean;
  enabled: boolean;
  /** 只读会话：模型选择器整块不渲染。 */
  readOnly: boolean;
  hintVisible: boolean;
  hintText: string;
  model: string;
  models: string[];
  effort: string;
  configBusy: boolean;
  configLocked: boolean;
  /** 主按钮就是停止键（输入框空着、正在跑）。 */
  showStop: boolean;
  /** 本轮可中断：停止入口至少要在某处出现。 */
  stopAvailable: boolean;
  sendable: boolean;
  busy: boolean;
  commandRunning: boolean;
  hasContent: boolean;
  working: boolean;
  /** 麦克风入口是否可用；紧凑子代理条不渲染。 */
  voiceAvailable: boolean;
  /** 麦克风是否处于采集态。 */
  voiceActive: boolean;
  /** 点击麦克风开始或结束。 */
  onToggleVoice: () => void;
  onPickAttachment: () => void;
  onModelChange?: (value: string) => void;
  onReasoningEffortChange?: (value: string) => void;
  onInterrupt: () => void;
  onSubmit: () => void;
}

export const ComposerActionBar: React.FC<ComposerActionBarProps> = ({
  compact,
  enabled,
  readOnly,
  hintVisible,
  hintText,
  model,
  models,
  effort,
  configBusy,
  configLocked,
  showStop,
  stopAvailable,
  sendable,
  busy,
  commandRunning,
  hasContent,
  working,
  voiceAvailable,
  voiceActive,
  onToggleVoice,
  onPickAttachment,
  onModelChange,
  onReasoningEffortChange,
  onInterrupt,
  onSubmit,
}) => {
  const reducedMotion = useReducedMotion();
  const activeButton = showStop || sendable || (commandRunning && !hasContent);
  const iconState = showStop
    ? 'stop'
    : busy || (commandRunning && !hasContent)
      ? 'busy'
      : 'send';
  /*
   * 停止入口常驻。
   *
   * 主按钮在输入框空着时就是停止键，此时不再多画一枚；一旦有内容（或提交在途），
   * 主按钮回到发送，停止改由它左边这枚 28px 图标承担，因此边打字边停手仍然有点可点。
   */
  const stopIcon = stopAvailable && !showStop;

  return (
    <div className="flex items-center gap-1 px-2.5 pb-2">
      {!compact && (
        <button
          type="button"
          aria-label="添加附件"
          title="添加附件"
          disabled={!enabled}
          onClick={onPickAttachment}
          className="shrink-0 w-7 h-7 rounded-sm flex items-center justify-center text-text-secondary transition-colors duration-100 active:scale-95 hover:bg-hover hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-40"
        >
          <PlusIcon size={16} />
        </button>
      )}

      {!compact && VOICE_UI_ENABLED && (
        <button
          type="button"
          aria-label={voiceActive ? '结束语音输入' : '开始语音输入'}
          aria-pressed={voiceActive}
          title={voiceActive ? '结束语音输入' : '开始语音输入'}
          disabled={!voiceAvailable}
          onClick={onToggleVoice}
          className={`shrink-0 w-7 h-7 rounded-sm flex items-center justify-center transition-colors duration-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus active:scale-95 disabled:cursor-not-allowed disabled:opacity-40 ${
            voiceActive
              ? 'bg-active text-text-primary'
              : 'text-text-secondary hover:bg-hover hover:text-text-primary'
          }`}
        >
          <MicrophoneIcon size={16} />
        </button>
      )}

      {hintVisible && (
        <span className="shrink-0 pl-1 text-2xs text-text-tertiary select-none">
          {hintText}
        </span>
      )}

      <span className="flex-1" />

      {/*
        模型选择按「这一轮由谁跑」显示：只读会话（目标执行这类后台会话）不能换模型，
        因此整块不渲染，除非存在别的锁（自动化会话由后端锁定）时才照常显示并置灰。
      */}
      {!compact && !readOnly && (enabled || model !== '') && (
        <ComposerPicker
          model={model}
          models={models}
          effort={effort}
          configBusy={configBusy}
          configLocked={configLocked}
          onModelChange={(value) => onModelChange?.(value)}
          onReasoningEffortChange={(value) => onReasoningEffortChange?.(value)}
        />
      )}

      {stopIcon && (
        <button
          type="button"
          aria-label="停止生成"
          title="停止生成（Esc）"
          onClick={onInterrupt}
          className="shrink-0 w-7 h-7 rounded-full flex items-center justify-center text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary active:scale-95 focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
        >
          <StopIcon size={14} />
        </button>
      )}

      <button
        type="button"
        aria-label={
          showStop
            ? '停止生成'
            : commandRunning && !hasContent
              ? '正在重载 MCP 服务器'
              : '发送'
        }
        title={
          showStop
            ? '停止生成（Esc）'
            : commandRunning && !hasContent
              ? '正在重载 MCP 服务器，请稍候'
              : working
                ? '发送：Agent 运行中，消息进入排队并在安全边界插入'
                : '发送'
        }
        disabled={showStop ? false : !sendable}
        onClick={showStop ? onInterrupt : onSubmit}
        className={`group shrink-0 w-8 h-8 rounded-full flex items-center justify-center transition-transform duration-100 active:scale-92 ${
          activeButton
            ? 'bg-primary text-white shadow-btn-primary hover:bg-primary-hover'
            : 'bg-surface-sunken text-text-tertiary opacity-40'
        } disabled:cursor-not-allowed`}
      >
        <AnimatePresence mode="wait" initial={false}>
          <motion.span
            key={iconState}
            initial={reducedMotion ? false : { opacity: 0, scale: 0.72 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={reducedMotion ? undefined : { opacity: 0, scale: 0.72 }}
            transition={reducedMotion ? { duration: 0 } : ICON_SPRING}
            className="inline-flex items-center justify-center"
          >
            {iconState === 'stop' ? (
              <StopIcon size={16} />
            ) : iconState === 'busy' ? (
              <span className="w-3.5 h-3.5 rounded-full border border-current border-t-transparent state-spinner" />
            ) : (
              <ArrowUpIcon
                size={16}
                className={sendable ? 'transition-transform duration-150 group-hover:-translate-y-0.5' : ''}
              />
            )}
          </motion.span>
        </AnimatePresence>
      </button>
    </div>
  );
};
