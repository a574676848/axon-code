import { call } from '../backend';
import { MAX_SPEECH_CHARS, VoiceFailure, type VoiceAudioPayload } from './controller';

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
 * 音色选择分区还没有落到桌面端，因此这里固定为空，由网关使用默认音色
 * （冷静知性）。等设置里有了音色，只改这一处：不在这里拼 speaker 或语速，
 * 那些参数属于合成侧的预设，前端不伪造。
 */
export function currentPresetId(): string | null {
  return null;
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
