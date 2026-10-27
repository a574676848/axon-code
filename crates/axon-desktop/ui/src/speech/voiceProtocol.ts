/*
 * 语音界面的协议口径：六态映射、服务端帧形状与定稿并稿规则。
 *
 * PROTOCOL 2.3 的相位全集有九个值，界面上只呈现六态：待机不占行，`error` 与
 * `closed` 用 Banner 与入口复位表达；`endpointing` 归入思考，因为对用户而言
 * 「说完话到出字」这一段是同一件事。播放与打断属于后续增量，`speaking` 与
 * `interrupted` 只在真的收到对应相位帧时出现，界面不自行捏造。
 */

/** 界面六态。 */
export type VoicePhase = 'idle' | 'arming' | 'listening' | 'thinking' | 'speaking' | 'interrupted';

/** 六态的中文文案；待机不占行，因此不在这里出现。 */
export const VOICE_PHASE_TEXT: Record<Exclude<VoicePhase, 'idle'>, string> = {
  arming: '准备中',
  listening: '聆听',
  thinking: '思考',
  speaking: '说话',
  interrupted: '被打断',
};

/** 服务端语音帧；只声明界面会用到的字段，其余字段原样忽略。 */
export interface VoiceServerFrame {
  type: string;
  phase?: string;
  text?: string;
  utteranceSeq?: number;
  code?: string;
  stage?: string;
  retryable?: boolean;
  message?: string;
  reason?: string;
}

/** 九个协议相位归到界面六态；未知相位按待机处理，不猜测。 */
export function phaseFromServerState(phase: string | undefined): VoicePhase {
  switch (phase) {
    case 'arming':
      return 'arming';
    case 'listening':
      return 'listening';
    case 'endpointing':
    case 'thinking':
      return 'thinking';
    case 'speaking':
      return 'speaking';
    case 'interrupted':
      return 'interrupted';
    default:
      return 'idle';
  }
}

/** 中日韩文字与全角标点；用来判断并稿时要不要补空格。 */
const CJK = /[\u3000-\u303f\u3400-\u9fff\uf900-\ufaff\uff00-\uffef]/;

/**
 * 把定稿并进当前草稿。
 *
 * 草稿是用户自己写的内容，语音定稿只能追加，不能覆盖；中文与中文之间不加空格，
 * 其余情况补一个空格，避免英文单词黏在一起。追加仍走编辑控制器的 `commit`，
 * 因此撤销一次就回到语音写入之前。
 */
export function joinDraft(current: string, addition: string): string {
  const next = addition.trim();
  if (next.length === 0) return current;
  if (current.length === 0) return next;
  if (/\s$/.test(current)) return `${current}${next}`;
  const left = current[current.length - 1];
  const right = next[0];
  return `${current}${CJK.test(left) && CJK.test(right) ? '' : ' '}${next}`;
}
