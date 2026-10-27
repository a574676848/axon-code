import { call } from '../backend';
import { MAX_SPEECH_CHARS, VoiceFailure, type VoiceAudioPayload } from './controller';
import { voicePreferences } from './preferences';

/*
 * 语音合成请求。
 *
 * 桌面端不持有网关凭据，也不从 WebView 直连网关：所有后端调用都走
 * `backend.ts` 的 IPC 通道，由宿主用配置里的凭据调 `/api/v1/voice/tts`。
 *
 * 契约：`voice_tts` 命令，入参 `{ text, presetId }`（预设为空表示默认音色），
 * 回执 `{ audioBase64, mimeType }`；失败是字符串，运行时未就绪以
 * `voice_unavailable:` 开头，其余文本原样展示。这一层只做转发与归类。
 */

/** 宿主侧合成命令；参数名由 Tauri 在 camelCase 与 snake_case 之间映射。 */
const SYNTHESIZE_COMMAND = 'voice_tts';

/**
 * 当前音色预设。
 *
 * 取自本地存储（`axon.voice.preset`）。空串表示跟随人格：这时返回 null，
 * 由后端按人格融合选音色；选过预设则把 id 原样带上，说话人、语速与音高
 * 都属于合成侧的预设表，前端不伪造。
 */
export function currentPresetId(): string | null {
  const presetId = voicePreferences.presetId();
  return presetId.length === 0 ? null : presetId;
}

/** 从宿主回执里取第一个非空字符串字段，兼容驼峰与下划线两种写法。 */
function stringField(record: Record<string, unknown>, ...keys: string[]): string {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === 'string' && value.length > 0) return value;
  }
  return '';
}

/** 宿主抛出的失败是字符串：运行时未就绪以 `voice_unavailable:` 开头，其余原样展示。 */
function classifyFailure(failure: unknown): VoiceFailure {
  const message =
    failure instanceof Error
      ? failure.message
      : typeof failure === 'string' && failure.length > 0
        ? failure
        : '语音合成失败';
  if (/^voice_unavailable\b/i.test(message) || /语音运行时未就绪|语音运行时不可用/.test(message)) {
    return new VoiceFailure('unavailable', '语音运行时未就绪');
  }
  return new VoiceFailure('failed', message);
}

/** 请求一段语音；失败一律抛 VoiceFailure，调用方按 kind 落状态。 */
export async function requestVoiceAudio(
  text: string,
  presetId: string | null,
): Promise<VoiceAudioPayload> {
  let raw: unknown;
  try {
    raw = await call<Record<string, unknown>>(SYNTHESIZE_COMMAND, {
      text: text.slice(0, MAX_SPEECH_CHARS),
      presetId,
    });
  } catch (failure) {
    throw classifyFailure(failure);
  }
  const record = (raw ?? {}) as Record<string, unknown>;
  const audioBase64 = stringField(record, 'audioBase64', 'audio_base64');
  if (audioBase64.length === 0) throw new VoiceFailure('failed', '语音合成没有返回音频');
  return {
    audioBase64,
    mimeType: stringField(record, 'mimeType', 'mime_type') || 'audio/wav',
  };
}

/*
 * 试听请求。
 *
 * 试听与朗读共用同一副嗓子，但失败口径更细：宿主把 504、409 与 400 分成
 * `preview_timeout:`、`voice_unavailable:` 与 `preview_invalid:` 三个前缀，
 * 这一层只把前缀翻译成分类与中文结论，界面不解析错误字符串。
 */

/** 试听失败分类。 */
export type VoicePreviewFailureKind = 'timeout' | 'unavailable' | 'invalid' | 'failed';

/** 试听超时的固定文案，与 PROTOCOL §4.2 的 `preview_timeout` 一致。 */
export const PREVIEW_TIMEOUT_MESSAGE = '试听超时，请重试或换一个音色';

/** 试听失败：带分类，调用方据此决定展示哪一句。 */
export class VoicePreviewFailure extends Error {
  readonly kind: VoicePreviewFailureKind;

  constructor(kind: VoicePreviewFailureKind, message: string) {
    super(message);
    this.name = 'VoicePreviewFailure';
    this.kind = kind;
  }
}

/** 把宿主抛出的前缀翻译成试听失败分类。 */
export function classifyPreviewFailure(failure: unknown): VoicePreviewFailure {
  const message =
    failure instanceof Error
      ? failure.message
      : typeof failure === 'string'
        ? failure
        : '';
  if (message.startsWith('preview_timeout:')) {
    return new VoicePreviewFailure('timeout', PREVIEW_TIMEOUT_MESSAGE);
  }
  if (message.startsWith('voice_unavailable:')) {
    const reason = message.slice('voice_unavailable:'.length).trim();
    return new VoicePreviewFailure('unavailable', reason || '该音色当前不可用');
  }
  if (message.startsWith('preview_invalid:')) {
    const reason = message.slice('preview_invalid:'.length).trim();
    return new VoicePreviewFailure('invalid', reason || '试听参数不正确');
  }
  return new VoicePreviewFailure('failed', message || '试听失败，请稍后重试');
}

/** 试听一个音色；`presetId` 为 null 表示跟随人格。 */
export async function requestVoicePreview(presetId: string | null): Promise<VoiceAudioPayload> {
  let raw: unknown;
  try {
    raw = await call<Record<string, unknown>>('voice_preview', { presetId });
  } catch (failure) {
    throw classifyPreviewFailure(failure);
  }
  const record = (raw ?? {}) as Record<string, unknown>;
  const audioBase64 = stringField(record, 'audioBase64', 'audio_base64');
  if (audioBase64.length === 0) throw new VoicePreviewFailure('failed', '试听没有返回音频');
  return {
    audioBase64,
    mimeType: stringField(record, 'mimeType', 'mime_type') || 'audio/wav',
  };
}
