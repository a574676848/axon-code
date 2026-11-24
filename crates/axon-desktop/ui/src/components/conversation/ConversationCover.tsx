import React from 'react';
import type { CoverFace } from '../../conversation/stage';
import { Z_CONTENT_OVERLAY } from './chatChrome';
import { ConversationHero } from './ConversationHero';
import { ConversationOpening } from './ConversationOpening';

/*
 * 正文上方的过渡面与首屏。
 *
 * 它挂在「正文舞台 + 输入坞」共用的外框上，而不是只挂在正文舞台内部：
 * 首屏因此按整列（含输入坞）的几何定位，坞长高时首屏不跟着上跳，
 * 也不再需要 `pt-[124px]` 这类凭经验补的重心偏移。
 *
 * 面本身只做透明度叠化；`pointer-events-none` 让它永远不挡住输入坞，
 * 挡住正文交互的是内容层的 `inert`。
 */

export type { CoverFace };

export interface ConversationCoverProps {
  /** 当前该露哪一屏；none 时整层透明。 */
  cover: CoverFace;
  /** 淡出期间保持上一份内容，避免叠化时先塌成空画布。 */
  content: 'hero' | 'skeleton';
}

export const ConversationCover: React.FC<ConversationCoverProps> = ({ cover, content }) => (
  <div
    data-conversation-stage="cover"
    data-cover={cover}
    className={`${Z_CONTENT_OVERLAY} absolute inset-0 flex flex-col bg-canvas pointer-events-none transition-opacity ease-out ${
      cover === 'none' ? 'duration-150 opacity-0' : 'duration-75 opacity-100'
    }`}
    inert={cover === 'none'}
    aria-hidden={cover === 'none'}
  >
    {content === 'skeleton' ? (
      <ConversationOpening />
    ) : (
      <div className="flex-1 min-h-0 flex flex-col items-center justify-center">
        <ConversationHero />
      </div>
    )}
  </div>
);
