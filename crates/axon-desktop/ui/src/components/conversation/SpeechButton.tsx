import React from 'react';
import { speak, speakableScript, stop, useSpeechState } from '../../speech';
import { SpeakerIcon, StopIcon } from '../../icons';
import { MessageActionButton } from './MessageFooter';

/*
 * 朗读按钮：挂在助手正文的操作区里，与复制、下载同一行。
 *
 * 同一时刻只有一条朗读，因此按钮读的是「当前念的是不是这一条」：
 * 合成在途时显示转圈（再点一次即取消），播放中换成停止图形。
 *
 * 「正在朗读」这件事由页脚的常显状态表达（声波加一行小字），不塞进这枚
 * 28px 的按钮：一个格子里同时画声波、进度和停止图形，读起来只是一团噪点。
 *
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
      <MessageActionButton label="停止朗读" title="正在准备朗读，再点一次取消" onClick={stop} pressed>
        <span className="h-3.5 w-3.5 animate-spin rounded-full border border-current border-t-transparent" />
      </MessageActionButton>
    );
  }

  if (playing) {
    return (
      <MessageActionButton label="停止朗读" title="正在朗读，再点一次停止" onClick={stop} pressed>
        <StopIcon size={13} />
      </MessageActionButton>
    );
  }

  const hint = unavailable ?? failed ?? '朗读这条回复';
  return (
    <MessageActionButton
      label="朗读这条回复"
      title={hint}
      pressed={false}
      disabled={unavailable !== null}
      tone={unavailable !== null ? 'gold' : failed !== null ? 'danger' : 'default'}
      onClick={() => speak(text, { source })}
    >
      <SpeakerIcon size={14} />
    </MessageActionButton>
  );
};
