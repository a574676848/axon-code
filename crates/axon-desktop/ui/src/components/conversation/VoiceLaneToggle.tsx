import React from 'react';
import { isSupported, useLaneAutoRead, useSpeechState, voicePreferences } from '../../speech';
import { SpeakerIcon, SpeakerOffIcon } from '../../icons';
import { VoiceWave } from './VoiceWave';

/*
 * 会话级自动朗读开关：放在指标条右侧的操作区里。
 *
 * 只写这一条会话自己的键，因此打开或关闭不会牵动任何别的会话；
 * 没有显式设置过的会话继承全局开关。按下态用选中底加主文本色，
 * 与列表行的选中语言一致，不新增色值。
 *
 * 正在朗读本会话时图标换成声波：这一格里就能看出声音确实在响，
 * 不必先成为「已经点开过」的那一页。
 */

export const VoiceLaneToggle: React.FC<{ laneKey: string | null }> = ({ laneKey }) => {
  const autoRead = useLaneAutoRead(laneKey);
  const speech = useSpeechState();
  if (laneKey === null || !isSupported()) return null;
  const sounding = speech.status === 'playing' || speech.status === 'preparing';
  return (
    <button
      type="button"
      aria-label="自动朗读此会话"
      aria-pressed={autoRead}
      data-voice-auto={sounding ? 'sounding' : autoRead ? 'on' : 'off'}
      title={autoRead ? '关闭此会话的自动朗读' : '开启此会话的自动朗读'}
      onClick={() => voicePreferences.setLaneAutoRead(laneKey, !autoRead)}
      className={`shrink-0 w-7 h-7 inline-flex items-center justify-center rounded-sm transition-colors duration-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus ${
        autoRead || sounding
          ? 'bg-active text-text-primary'
          : 'text-text-tertiary hover:bg-hover hover:text-text-primary'
      }`}
    >
      {sounding ? <VoiceWave active size="xs" /> : autoRead ? <SpeakerIcon size={14} /> : <SpeakerOffIcon size={14} />}
    </button>
  );
};
