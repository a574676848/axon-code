import React from 'react';
import { isSupported, useLaneAutoRead, voicePreferences } from '../../speech';
import { SpeakerIcon } from '../../icons';

/*
 * 会话级自动朗读开关：放在指标条右侧的操作区里。
 *
 * 只写这一条会话自己的键，因此打开或关闭不会牵动任何别的会话；
 * 没有显式设置过的会话继承全局开关。按下态用选中底加主文本色，
 * 与列表行的选中语言一致，不新增色值。
 */

export const VoiceLaneToggle: React.FC<{ laneKey: string | null }> = ({ laneKey }) => {
  const autoRead = useLaneAutoRead(laneKey);
  if (laneKey === null || !isSupported()) return null;
  return (
    <button
      type="button"
      aria-label="自动朗读此会话"
      aria-pressed={autoRead}
      title={autoRead ? '关闭此会话的自动朗读' : '开启此会话的自动朗读'}
      onClick={() => voicePreferences.setLaneAutoRead(laneKey, !autoRead)}
      className={`shrink-0 w-7 h-7 inline-flex items-center justify-center rounded-sm transition-colors duration-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus ${
        autoRead
          ? 'bg-active text-text-primary'
          : 'text-text-tertiary hover:bg-hover hover:text-text-primary'
      }`}
    >
      <SpeakerIcon size={14} />
    </button>
  );
};
