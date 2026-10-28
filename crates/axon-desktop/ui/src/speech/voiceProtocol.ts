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
  /** `turn` 帧：网关为本轮分配的 Interaction 标识。 */
  interactionId?: string;
  /** `turn` 与 `stopAudio` 帧：会话内轮次。 */
  epoch?: number;
  /** `audio` 帧：base64 编码的 16 kHz 单声道 s16le PCM。 */
  pcmBase64?: string;
  /** `audio` 帧：宿主包序号，原样透传。 */
  seq?: number;
  /** `error` 帧里的累计丢帧数。 */
  detail?: { droppedFrames?: number };
}

/** 网关回执类错误码：会话可以继续，不该把入口复位。 */
export function isRecoverableVoiceError(code: string | undefined): boolean {
  return code === 'backpressure';
}

/**
 * 语音错误码到可执行结论。
 *
 * 与成熟语音输入产品一致：失败必须给出下一步动作，而不是把内部错误码或
 * 一句「出现内部错误」丢给用户。未知错误码才回落到服务端文案。
 */
const VOICE_ERROR_TEXT: Record<string, string> = {
  model_missing: '本地语音模型尚未安装，重新运行 Axon 安装脚本即可补齐',
  model_unusable: '本地语音模型不可用，检查语音运行时安装后重试',
  worker_unavailable: '语音引擎没能启动，稍后重试或重启 Axon',
  worker_crashed: '语音引擎意外退出，本次听写已停止',
  protocol_sequence: '语音会话状态异常，请重新开始',
  frame_oversize: '音频帧异常，本次听写已停止',
  session_idle: '语音会话长时间无操作，已自动结束',
  tts_failed: '本机朗读合成失败，稍后重试',
  preset_unavailable: '当前音色不可用，换一个音色再试',
  voice_unavailable: '语音运行时未就绪，完成语音依赖安装后重试',
};

/** 把错误码翻成一句可执行的中文；未知码用服务端文案兜底。 */
export function describeVoiceError(code: string | undefined, fallback: string): string {
  if (code !== undefined) {
    const text = VOICE_ERROR_TEXT[code];
    if (text !== undefined) return text;
  }
  return fallback;
}

/** 六个界面相位里属于「机器在忙」的那几档，用于状态行计时与打断入口。 */
export function isBusyPhase(phase: VoicePhase): boolean {
  return phase === 'arming' || phase === 'thinking' || phase === 'speaking';
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
