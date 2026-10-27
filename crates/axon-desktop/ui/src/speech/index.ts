import { speech } from './instance';

/*
 * 语音播报：桌面端的对外入口。
 *
 * `speak(text, { onEnd })` 朗读一段正文，`stop()` 停止当前朗读，
 * `isSupported()` 回答这个 WebView 能不能播放音频。朗读文本一律先过
 * `speakableScript` 净化，因此调用方传消息原文即可。
 */

export { MAX_SPEECH_CHARS, VoiceFailure } from './controller';
export type { SpeechState, SpeechStatus, VoiceAudioPayload } from './controller';
export { speakableScript } from './sanitize';
export { voicePreferences } from './preferences';
export { VOICE_STORAGE_KEYS } from './keys';
export { FOLLOW_PERSONA_ID, parseVoicePresets, voicePresetRows } from './presets';
export type { VoicePreset, VoicePresetRow, VoicePresetsPayload } from './presets';
export {
  useGlobalAutoRead,
  useLaneAutoRead,
  useSpeechState,
  useStopSpeechOnLaneChange,
  useVoicePresetId,
} from './hooks';

/** 开始朗读一条正文；同一时刻只保留一条，新的会先停掉旧的。 */
export const speak = speech.speak;
/** 停止当前朗读，并释放这段音频占用的地址。 */
export const stop = speech.stop;
/** 当前朗读状态快照。 */
export const speechState = speech.state;
/** 订阅朗读状态变化。 */
export const subscribeSpeech = speech.subscribe;

/** 这个 WebView 是否具备播放能力；不具备时界面不画喇叭按钮。 */
export function isSupported(): boolean {
  return (
    typeof Audio !== 'undefined' &&
    typeof URL !== 'undefined' &&
    typeof URL.createObjectURL === 'function'
  );
}
