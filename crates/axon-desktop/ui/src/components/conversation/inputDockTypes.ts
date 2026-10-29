import type { AttachmentRef } from '../../conversation/transcript';
import type { AnswerItem, PendingQuestion } from '../../conversation/questions';
import type { McpCommandReceipt } from '../../conversation/mcpReading';
import type { VoiceEntry } from './useComposerVoice';

/*
 * 输入坞的对外契约。
 *
 * 类型与实现分开放：输入坞的实现已经很长，而这份契约的读者主要是调用方，
 * 单独成文之后两边都不必在对方的行数里找自己关心的那一段。
 */
export interface InputDockProps {
  /** 当前工作区根目录；`@` 文件引用按它做作用域索引。 */
  workspaceRoot: string | null;
  /** 当前工作区标识；语音会话首帧需要它。 */
  workspaceId?: string | null;
  /** 当前会话的回复目的地；语音会话首帧需要它。 */
  replyDestination?: string;
  /**
   * 语音入口的会话目标：首屏没有车道时由上层临时建一条。
   *
   * 输入坞固定是听写：识别结果只写进草稿，绝不直接提交成一轮对话。
   */
  voiceTarget?: {
    /** 首屏起语音时先建会话，返回可用的工作区与目的地。 */
    ensureSession?: () => Promise<VoiceEntry | null>;
  };
  /** 是否可提交。 */
  enabled: boolean;
  /** 正在提交。 */
  busy: boolean;
  /**
   * 紧凑变体：子代理输入条共用这一份实现。
   *
   * 只关掉与主会话无关的两项（模型选择与附件），输入、提交、键盘与历史全部沿用，
   * 因此不会再出现第三套字号与发送按钮。
   */
  compact?: boolean;
  /** 键盘提示是否显示：列宽不足时收起，避免那一行先折行再被裁。 */
  hintVisible?: boolean;
  /** 占位文案覆盖（子代理输入条要给「把下一步交给谁」）。 */
  placeholder?: string;
  /** 卡片下方提示文案覆盖。 */
  hint?: string;
  /** 当前主模型；紧凑变体不需要。 */
  model?: string;
  /** 可选模型清单。 */
  models?: string[];
  /** 配置写入期间暂停模型与推理等级切换。 */
  configBusy?: boolean;
  configLocked?: boolean;
  /** 当前推理等级。 */
  reasoningEffort?: string | null;
  /** 切换主模型。 */
  onModelChange?: (value: string) => void;
  /** 切换推理等级。 */
  onReasoningEffortChange?: (value: string) => void;
  /**
   * 执行一条内置指令（`/mcp` 这类不进会话的本机动作），返回回执与可展开的清单。
   *
   * 不接这根线时指令不会被吞掉，正文原样留给会话。
   */
  onRunCommand?: (name: string) => Promise<McpCommandReceipt>;
  /** 提交正文。 */
  onSubmit: (text: string, attachments: AttachmentRef[], structuredInput: string) => void;
  /** 外部错误提示。 */
  error: string | null;
  /** 工作状态行：与 TUI 的 composer dock 同位置同口径；为空时不渲染。 */
  working: { primary: string; elapsedMs: number | null; queued: number } | null;
  /** 中断正在运行的一轮。 */
  onInterrupt: () => void;
  /**
   * 当前会话标识（工作区与会话拼成）。
   *
   * 问答占座卡只在「这一次进来」里听从收起：换一条会话再回来，还没答完的提问
   * 要自己回到占位位，因此卡片状态需要认出会话换没换。
   */
  laneKey?: string | null;
  /** 当前待决或挂起的模型提问。 */
  activeQuestion?: PendingQuestion | null;
  /** 提交问答作答。 */
  onAnswerQuestion?: (callId: string, answers: AnswerItem[]) => void;
  /**
   * 重新打开卡片的请求：外部入口（时间线上的「重新打开」）每点一次就递增一次，
   * 卡片据此从收起态回到输入坞的主展示位。
   */
  reopenToken?: number;
  /**
   * 作答提交失败的原因（例如提问已在别处结算）。
   *
   * 这类失败是服务端对一次操作的拒绝，不是输入坞的常态提示，因此用弹窗告知，
   * 不占用输入框上方的错误位。
   */
  questionError?: string | null;
  /** 关闭作答失败弹窗。 */
  onDismissQuestionError?: () => void;
}
