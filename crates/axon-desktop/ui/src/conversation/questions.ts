/**
 * 用户问答交互协议常量、类型与正文剥离规则（UQ-009, UQ-104, UQ-108）。
 */

/** 倒计时告警阈值（毫秒）：剩余 <= 30 秒转琥珀色告警。 */
export const COUNTDOWN_WARN_THRESHOLD_MS = 30_000;

/** 倒计时警报阈值（毫秒）：剩余 <= 10 秒转红色警报。 */
export const COUNTDOWN_DANGER_THRESHOLD_MS = 10_000;

/** 默认让位交接文案；与宿主默认文案同措辞，仅用于宿主未下发时的兜底展示。 */
export const DEFAULT_SUSPENDED_HANDOFF =
  '我需要你的回答才能继续这一步。你随时回答都可以，收到后我会接着往下做。';

/** 结构化回执块标识。 */
export const QUESTION_RECEIPT_BLOCK_HEADER = '[问答回执]';
export const QUESTION_RECEIPT_CALL_PREFIX = '回答待决问题：';
export const QUESTION_RECEIPT_CONTENT_PREFIX = '回答内容：';

/** 问答选项。 */
export interface QuestionOption {
  label: string;
  description?: string | null;
}

/** 单个问题定义。 */
export interface QuestionItem {
  id: string;
  question: string;
  header?: string | null;
  detail?: string | null;
  options?: QuestionOption[];
  multiSelect?: boolean;
}

/** 用户回答项。 */
export interface AnswerItem {
  id: string;
  selected: string[];
  custom?: string | null;
}

/** 问答生命周期状态。 */
export type QuestionStatus = 'pending' | 'extended' | 'settled' | 'suspended';

/** 客户端内存中的待决/已结算提问状态。 */
export interface PendingQuestion {
  partId: string;
  callId: string;
  status: QuestionStatus;
  questions: QuestionItem[];
  answers?: AnswerItem[];
  handoff?: string | null;
  deadlineAtMs: number;
}

/**
 * 对账快照：Rust 侧适配后的网关记录，字段与 `PendingQuestion` 一一对应。
 *
 * 适配在 Rust 侧完成（`state` 翻译成状态、作答内容取 `answer`、缺失的 partId
 * 按 callId 派生），前端因此不必自己解释网关协议，也不会把服务端字段直接覆盖
 * 到本地结构上而抹掉真正的状态与标识。
 */
export type QuestionSnapshot = PendingQuestion;

/** 计算当前倒计时分级阶段。 */
export function getCountdownPhase(remainingMs: number): 'normal' | 'warning' | 'danger' {
  if (remainingMs <= COUNTDOWN_DANGER_THRESHOLD_MS) {
    return 'danger';
  }
  if (remainingMs <= COUNTDOWN_WARN_THRESHOLD_MS) {
    return 'warning';
  }
  return 'normal';
}

/**
 * 剥离消息正文里的结构化问答回执块，仅保留可见正文。
 *
 * 回执块的唯一编码格式与后端逐字一致：一段可见摘要之后接标题行、`回答待决问题：`
 * 与 `回答内容：` 两行（摘要为空时以标题行开头）。因此这里按**整块校验**再剥离：
 * 标题必须独占一行、后面两行的引导前缀与取值都要成立。正文里恰好写了同名标题
 * （例如用户自己复述这段格式）不会被误剥。
 */
export function stripQuestionReceiptBlock(content: string): string {
  const lines = content.split('\n');
  for (let start = lines.length - 1; start >= 0; start -= 1) {
    if (lines[start].trim() !== QUESTION_RECEIPT_BLOCK_HEADER) continue;
    if (!isReceiptBlockBody(lines.slice(start + 1))) continue;
    return lines.slice(0, start).join('\n').trimEnd();
  }
  return content;
}

/** 标题之后的内容是否构成一个完整的回执块。 */
function isReceiptBlockBody(lines: readonly string[]): boolean {
  let callAt = -1;
  for (let index = 0; index < lines.length; index += 1) {
    if (lines[index].trim().length === 0) continue;
    callAt = index;
    break;
  }
  if (callAt < 0) return false;
  const call = lines[callAt].trim();
  if (!call.startsWith(QUESTION_RECEIPT_CALL_PREFIX)) return false;
  if (call.slice(QUESTION_RECEIPT_CALL_PREFIX.length).trim().length === 0) return false;
  // 作答内容是压缩过的单行 JSON，但也允许它自带换行：取引导行之后的全部文本。
  const rest = lines.slice(callAt + 1).join('\n').trim();
  if (!rest.startsWith(QUESTION_RECEIPT_CONTENT_PREFIX)) return false;
  return rest.slice(QUESTION_RECEIPT_CONTENT_PREFIX.length).trim().length > 0;
}

/**
 * 清掉已结算的提问状态。
 *
 * 待答位只关心「还需要用户回答」的那几条；结算过的条目留在映射里既没有消费者，
 * 也会随会话时长无界增长。`keepCallId` 是本次新到的提问，它永远保留；不给时
 * 只按状态过滤，调用方因此既能「留下刚到的那条」也能「一律清干净」。
 */
export function pruneSettledQuestions(
  questions: Readonly<Record<string, PendingQuestion>>,
  keepCallId: string | null = null,
): Record<string, PendingQuestion> {
  const kept = Object.entries(questions).filter(
    ([callId, question]) => callId === keepCallId || question.status !== 'settled',
  );
  if (kept.length === Object.keys(questions).length) return questions as Record<string, PendingQuestion>;
  return Object.fromEntries(kept);
}

/** 答案的投递方式：直接回灌给等待中的工具调用，同一轮继续。 */
export const ANSWER_DELIVERY_TOOL_RETURN = 'tool_return';

/**
 * 这次作答之后要不要把车道重新贴一次。
 *
 * 答案回灌给活等待者时，界面手上那条订阅还在看着同一轮，后续帧自己会到；
 * 走消息通道时答案变成车道上的一条新用户消息，守护进程据此另开一轮，而界面
 * 在此之前并不知道有这条 Interaction，不重贴就会停在原地，看起来像「答了但没继续」。
 *
 * 缺省（网关未下发投递方式）按需要重贴处理：少贴一次等于界面停在原地，
 * 多贴一次只是把车道重放一遍。
 */
export function shouldReattachLane(deliveryMode: string | null | undefined): boolean {
  return deliveryMode !== ANSWER_DELIVERY_TOOL_RETURN;
}

/** 从消息中解析出结构化问答回执内容。 */
export function extractQuestionReceipt(
  content: string,
): { callId: string; answersJson: string } | null {
  const index = content.indexOf(QUESTION_RECEIPT_BLOCK_HEADER);
  if (index === -1) {
    return null;
  }
  const block = content.slice(index + QUESTION_RECEIPT_BLOCK_HEADER.length);
  let callId: string | null = null;
  let answersJson: string | null = null;

  for (const line of block.split('\n')) {
    const trimmed = line.trim();
    if (trimmed.startsWith(QUESTION_RECEIPT_CALL_PREFIX)) {
      callId = trimmed.slice(QUESTION_RECEIPT_CALL_PREFIX.length).trim();
    } else if (trimmed.startsWith(QUESTION_RECEIPT_CONTENT_PREFIX)) {
      answersJson = trimmed.slice(QUESTION_RECEIPT_CONTENT_PREFIX.length).trim();
    }
  }

  if (callId && answersJson) {
    return { callId, answersJson };
  }
  return null;
}
