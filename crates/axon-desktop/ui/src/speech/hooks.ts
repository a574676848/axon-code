import React from 'react';
import { speech } from './instance';
import { VOICE_UI_ENABLED } from './enabled';
import { voicePreferences } from './preferences';
import type { SpeechState } from './controller';

/*
 * 语音播报的 React 接口。
 *
 * 状态与开关都存在模块里（朗读只有一份、偏好落在本地存储），
 * 因此组件用 useSyncExternalStore 订阅，不另建一层上下文。
 *
 * 语音入口隐藏期间（本机模型已移除、云端提供方尚未接入），自动朗读一律视为关闭：
 * 界面上的开关全部收起，后台也不该再发起合成请求。
 */

/** 朗读状态：静息、准备、播放、不可用与失败都由这一份快照驱动。 */
export function useSpeechState(): SpeechState {
  return React.useSyncExternalStore(speech.subscribe, speech.state, speech.state);
}

/** 全局开关（系统设置里的那一枚）。 */
export function useGlobalAutoRead(): boolean {
  return React.useSyncExternalStore(
    voicePreferences.subscribe,
    () => VOICE_UI_ENABLED && voicePreferences.globalAutoRead(),
    () => false,
  );
}

/** 这一条会话是否自动朗读：没有显式设置时继承全局值。 */
export function useLaneAutoRead(laneKey: string | null): boolean {
  return React.useSyncExternalStore(
    voicePreferences.subscribe,
    () => VOICE_UI_ENABLED && voicePreferences.effectiveAutoRead(laneKey),
    () => false,
  );
}

/** 当前音色预设 id；空串表示跟随人格。 */
export function useVoicePresetId(): string {
  return React.useSyncExternalStore(
    voicePreferences.subscribe,
    () => voicePreferences.presetId(),
    () => '',
  );
}

/**
 * 切换会话或离开主区域时停止朗读。
 *
 * 上一条会话的声音不该跟到新会话里，窗口关掉之后也不该还有一段音频在念。
 */
export function useStopSpeechOnLaneChange(laneKey: string | null): void {
  React.useEffect(() => () => speech.stop(), [laneKey]);
}
