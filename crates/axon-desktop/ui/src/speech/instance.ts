import { createSpeechController } from './controller';
import { currentPresetId, requestVoiceAudio } from './client';
import { createAudioPlayer } from './audioPlayer';

/*
 * 朗读实例：全应用一份。
 *
 * 合成请求走宿主 IPC，播放走 WebView 的 Audio 与 Blob 地址，两者在这里装配，
 * 于是「同一时刻只念一条」只有这一份状态。
 */

export const speech = createSpeechController({
  requestAudio: requestVoiceAudio,
  createPlayer: createAudioPlayer,
  presetId: currentPresetId,
});
