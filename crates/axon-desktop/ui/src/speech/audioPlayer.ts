import type { PlayerEvents, VoiceAudioPayload, VoicePlayerHandle } from './controller';

/*
 * 浏览器播放器：把 base64 音频做成 Blob 地址交给 Audio。
 *
 * 合成结果是真实的 wav 字节，不能按文本解码，否则二进制里的高位会被换掉。
 * 停止与收口都必须释放地址：不释放的话解码后的音频会一直占着内存，
 * 反复朗读几百条消息就是几百份音频留在那里。
 */

/** base64 还原成原始字节。 */
function decodeBase64(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

export function createAudioPlayer(
  payload: VoiceAudioPayload,
  events: PlayerEvents,
): VoicePlayerHandle {
  const blob = new Blob([decodeBase64(payload.audioBase64) as unknown as BlobPart], {
    type: payload.mimeType,
  });
  const url = URL.createObjectURL(blob);
  const audio = new Audio(url);
  let released = false;

  const release = (): void => {
    if (released) return;
    released = true;
    audio.onended = null;
    audio.onerror = null;
    URL.revokeObjectURL(url);
  };

  audio.onended = () => {
    release();
    events.onEnded();
  };
  audio.onerror = () => {
    release();
    events.onFailed(new Error('音频播放失败'));
  };

  return {
    play: () => audio.play(),
    stop: () => {
      audio.pause();
      release();
    },
  };
}
