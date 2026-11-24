import React from 'react';
import { BUBBLE_RADIUS } from './chatChrome';

/*
 * 会话打开时的过渡面。
 *
 * 切换会话要读历史索引、再逐条回放正文，这段窗口里没有可读的内容。直接留白就是
 * 那一下白闪，逐段画出来又是一串跳动，因此慢过阈值时用它占位：与正文列同宽、同间距、
 * 同节奏，回放落地后整层叠化收起，版面不跳。
 *
 * 骨架只取版面节奏，不复刻正文：它表达「这一屏正在打开」，不假装内容已经在了。
 */

/** 骨架条与用户气泡取正文自己的取值，叠化收起时形状与颜色都对得上。 */
export const ConversationOpening: React.FC = () => (
  <div className="flex-1 min-h-0 overflow-hidden px-5 pt-4 pb-6" aria-hidden="true">
    {/* 呼吸走整列一个动画：逐条各自动一次只会让画面更吵。 */}
    <div className="w-full max-w-chat mx-auto flex flex-col gap-2.5 animate-pulse">
      <div className="flex justify-end">
        <span className="w-2/5 h-11 bg-bubble" style={{ borderRadius: BUBBLE_RADIUS }} />
      </div>
      <div className="flex flex-col gap-2">
        <span className="h-3 w-11/12 rounded-full bg-border-strong" />
        <span className="h-3 w-4/5 rounded-full bg-border-strong" />
        <span className="h-3 w-3/5 rounded-full bg-border-strong" />
      </div>
    </div>
  </div>
);
