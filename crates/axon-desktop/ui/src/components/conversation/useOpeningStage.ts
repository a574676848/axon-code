import React from 'react';

/** 打开慢过这个时长才亮过渡面：快到看不出来时上一屏继续显示，不做多余的闪动。 */
const OPENING_SURFACE_DELAY_MS = 140;
/** 过渡面亮起后至少停留的时长：才亮就撤会读成一次闪动。 */
const OPENING_SURFACE_MIN_MS = 160;

/** 过渡相位的两个事实。 */
export interface OpeningStage {
  /** 回放还没落地，或过渡面还没停够时长。 */
  settling: boolean;
  /** 过渡面是否已亮起；`coverFace` 据此决定要不要给出过渡面。 */
  slowOpening: boolean;
}

/**
 * 过渡相位：回放还没落地，或过渡面还没停够时长。
 *
 * 后半句是必需的：慢了才亮、亮了就撤，会让过渡面自己变成一次闪动。停够时长再交出
 * 正文，回放的快慢就只体现在它停留的长短上。
 */
export function useOpeningStage(opening: boolean, identity: string | null): OpeningStage {
  const [slowOpening, setSlowOpening] = React.useState(false);
  const [coverHeld, setCoverHeld] = React.useState(false);
  /*
   * 只有「换了一条会话」才算换屏。
   *
   * 跨渠道事件（后台任务完成、子代理落地、目标推进、cron 唤醒主 turn）会让投影重新
   * 走一遍回放，于是 `opening` 又一次为真；若照样算作换屏，正文就会每来一条外部事件
   * 闪一次过渡面。身份没变时它是同一条会话的增量更新，正文原地长出来即可。
   */
  const [settledIdentity, setSettledIdentity] = React.useState<string | null>(null);
  const switched = opening && identity !== settledIdentity;
  React.useEffect(() => {
    if (!opening) {
      if (identity !== null) setSettledIdentity(identity);
      return;
    }
    if (!switched) return undefined;
    const show = window.setTimeout(() => setSlowOpening(true), OPENING_SURFACE_DELAY_MS);
    const hold = window.setTimeout(
      () => setCoverHeld(true),
      OPENING_SURFACE_DELAY_MS + OPENING_SURFACE_MIN_MS,
    );
    return () => {
      window.clearTimeout(show);
      window.clearTimeout(hold);
    };
  }, [opening, switched, identity]);
  React.useEffect(() => {
    if (switched) return;
    setSlowOpening(false);
    setCoverHeld(false);
  }, [switched]);
  return { settling: switched || (slowOpening && !coverHeld), slowOpening };
}
