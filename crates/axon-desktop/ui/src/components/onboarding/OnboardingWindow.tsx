import React from 'react';
import { WindowChrome } from '../ui/WindowChrome';
import { OnboardingFlow } from './OnboardingFlow';

/*
 * 零档案时的整屏引导。
 *
 * 它沿用界面兜底页 `error.html` 的形态：整屏一页、居中卡片、画布底色、同一套令牌。
 * 窗口里只渲染引导本身，侧栏、连接胶囊、命令面板与设置面板一块都不存在，
 * 因此没有可点穿的路径，也不需要半透明遮罩去压暗背后的东西。窗口三键留在舞台
 * 顶栏里，引导期间照样能最小化与关闭。
 */
export const OnboardingWindow: React.FC<{ onDone: () => void }> = ({ onDone }) => (
  <div className="app-shell-tray h-full w-full flex overflow-clip">
    <div className="relative flex-1 min-w-0 my-1.5 mx-1.5 flex flex-col rounded-[12px] bg-canvas hairline border-border-hairline overflow-clip">
      <WindowChrome />
      <span aria-hidden="true" className="noise-underlay absolute inset-0" />
      <div className="flex-1 min-h-0 min-w-0 flex">
        <OnboardingFlow onDone={onDone} />
      </div>
    </div>
  </div>
);
