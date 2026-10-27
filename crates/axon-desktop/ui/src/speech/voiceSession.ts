import { call, isDesktopHost } from '../backend';
import type { VoiceServerFrame } from './voiceProtocol';

/*
 * 语音会话的宿主通道。
 *
 * 命令名与事件名必须与 Rust 侧一致：`voice_start` 建连并等首个 ready，
 * `voice_stop` 结束当前句，`axon://voice` 下发服务端帧。浏览器调试桥没有
 * 原始字节通道，因此语音采集只在桌面宿主内提供；这里对非宿主返回空订阅。
 */

/** 与 Rust 侧 `VOICE_EVENT` 一致的事件名。 */
const VOICE_EVENT = 'axon://voice';

/** 开始一轮语音会话；返回时连接已就绪。 */
export async function startVoiceSession(
  workspaceId: string,
  replyDestination: string,
): Promise<void> {
  await call('voice_start', { workspaceId, replyDestination });
}

/** 结束当前句；服务端随后回 `final`。 */
export async function stopVoiceSession(): Promise<void> {
  await call('voice_stop');
}

/** 订阅服务端语音帧；非桌面宿主返回空订阅。 */
export async function listenVoiceSession(
  handler: (frame: VoiceServerFrame) => void,
): Promise<() => void> {
  if (!isDesktopHost()) return () => undefined;
  const { listen } = await import('@tauri-apps/api/event');
  return listen<VoiceServerFrame>(VOICE_EVENT, (event) => handler(event.payload));
}
