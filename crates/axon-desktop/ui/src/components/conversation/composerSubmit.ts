import type { McpCommandReceipt } from '../../conversation/mcpReading';
import type { AttachmentRef } from '../../conversation/transcript';

/*
 * 输入坞的提交闭环：把草稿、附件与链接占位收敛成一次「发出去」。
 *
 * 它与渲染无关，因此从组件里搬出来：输入坞本身的行数已经很紧，
 * 而这一段的分支（本机指令不进会话、占位展开回原文、历史写入失败的回执）
 * 又需要一个独立的地方讲清楚。
 */

/** 提交所需的最小依赖面。 */
export interface ComposerSubmitWiring {
  /** 当前正文。 */
  text: string;
  /** 当前附件。 */
  attachments: readonly AttachmentRef[];
  /** 有可发内容（正文或附件）。 */
  hasContent: boolean;
  /** 正在提交。 */
  busy: boolean;
  /** 输入坞是否可用。 */
  enabled: boolean;
  /** 把链接占位展开回原始地址。 */
  expandLinks: (text: string) => string;
  /** 组装结构化输入块。 */
  buildStructuredInput: (text: string, attachmentPaths: string[]) => string;
  /** 执行一条本机指令；未接线时指令按普通正文处理。 */
  runCommand?: (name: string) => Promise<McpCommandReceipt>;
  /** 派发本机指令。 */
  runLocalCommand: (name: string) => void;
  /** 清空草稿与全部候选、历史状态。 */
  reset: () => void;
  /** 记一条输入历史；失败时给出回执。 */
  recordHistory: (text: string) => void;
  /** 真正发出去。 */
  send: (text: string, attachments: AttachmentRef[], structuredInput: string) => void;
  /** 判定正文是否恰好是一条内置指令。 */
  matchCommand: (text: string) => { name: string } | null;
}

/** 生成提交动作；语义与界面上那一枚发送键逐条一致。 */
export function createComposerSubmit(wiring: ComposerSubmitWiring): () => void {
  const {
    text,
    attachments,
    hasContent,
    busy,
    enabled,
    expandLinks,
    buildStructuredInput,
    runCommand,
    runLocalCommand,
    reset,
    recordHistory,
    send,
    matchCommand,
  } = wiring;

  return () => {
    if (!hasContent || busy || !enabled) return;
    /*
     * 内置指令是界面自己的动作，不进会话：草稿清空，回执与清单都落在卡片上方。
     * 只认「正文恰好是一条指令」；带附件时照常发给模型，没有接执行器时按普通正文处理。
     */
    const matched = attachments.length === 0 ? matchCommand(text) : null;
    if (matched !== null && runCommand !== undefined) {
      reset();
      runLocalCommand(matched.name);
      return;
    }
    // 引用的正文照旧展示给用户，只有真正发出去的正文带上结构化输入块。
    // 链接占位在这里展开回原始地址：模型与消息记录拿到的都是一段完整 URL。
    const outgoing = expandLinks(text);
    // 附件同样进结构化输入块：只有附件、没有正文时，它就是这条消息的正文。
    const paths = attachments.map((attachment) => attachment.path);
    send(outgoing, [...attachments], buildStructuredInput(outgoing, paths));
    // 与 TUI 同一份历史：记下来，之后按上键就能取回（含发送失败的那一次）。
    recordHistory(outgoing);
    reset();
  };
}
