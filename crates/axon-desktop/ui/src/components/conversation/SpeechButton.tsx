import React from 'react';
import { speak, speakableScript, stop, useSpeechState } from '../../speech';
import { SpeakerIcon, StopIcon } from '../../icons';
import { MessageActionButton } from './MessageFooter';

/*
 * 朗读按钮：挂在助手正文的操作区里，与复制、下载同一行。
 *
 * 同一时刻只有一条朗读，因此按钮读的是「当前念的是不是这一条」：
 * 播放中翻成停止态，再点一次即停止；合成在途时同样显示停止态，避免连点发出两条请求。
 * 语音运行时未就绪时按钮转为琥珀并置灰，提示写明原因，由控制器在重试窗口后放开；
 * 全程不退回系统朗读。
 */

export const SpeechButton: React.FC<{ text: string; source: string }> = ({ text, source }) => {
  const state = useSpeechState();
  const active = state.source === source;
  const playing = active && (state.status === 'playing' || state.status === 'preparing');
  const preparing = playing && state.status === 'preparing';
  const failed = active && state.status === 'error' ? state.message : null;
  const unavailable = state.status === 'unavailable' ? (state.message ?? '语音运行时未就绪') : null;

  // 只有代码块的回复没有可念的文字：不画一枚点了没反应的按钮。
  if (speakableScript(text).length === 0) return null;

  if (preparing) {
    return (
      <MessageActionButton label="停止朗读" title="正在准备朗读" onClick={stop} pressed>
        <SpeakerIcon size={14} />
      </MessageActionButton>
    );
  }

  const hint = playing
    ? '停止朗读'
    : unavailable !== null
      ? unavailable
      : (failed ?? '朗读这条回复');
  return (
    <MessageActionButton
      label={playing ? '停止朗读' : '朗读这条回复'}
      title={hint}
      pressed={playing}
      disabled={unavailable !== null}
      tone={
        playing
          ? 'active'
          : unavailable !== null
            ? 'gold'
            : failed !== null
              ? 'danger'
              : 'default'
      }
      onClick={() => {
        if (playing) stop();
        else speak(text, { source });
      }}
    >
      {playing ? <StopIcon size={14} /> : <SpeakerIcon size={14} />}
    </MessageActionButton>
  );
};
