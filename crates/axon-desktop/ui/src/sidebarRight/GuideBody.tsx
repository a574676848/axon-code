import React from 'react';
import * as ipc from '../ipc';
import type { ShellChoiceItem } from '../types';
import { ChevronDownIcon } from '../icons';
import { Dropdown } from '../components/ui/overlays';
import { CompassIcon, CubeIcon, GuideArtworkFiles, GuideArtworkTerminal } from './guideArtwork';
import { guideEntries, type GuideArtworkId } from './registry';
import type { RailController } from './useRailLayout';

/*
 * 引导页：一面罗盘压在几个入口之上，居中摆在面板里，别的什么都没有。
 *
 * 不写标题：这是一扇门，不是一页说明，浏览器起始页也不会在自己的门上面加一句介绍。
 * 入口由各 tab 类型自己声明（见 `registry.ts` 的 `guide`），这里只按 `order` 摆；
 * 点一个入口就把那个类型开在引导页的位置上（引导页是门，不是留下来的那一页）。
 *
 * 底色用下沉面：停靠栏的面板是纯白，引导页退半档之后，「一列内容」与「一扇门」
 * 才分得开；否则白卡压在白底上只剩一条 12% 的发丝线可读。
 *
 * 末端的 10% 撑条把整列抬到面板中线略上方：正中的排版读起来会偏下，
 * 与文件浏览的空态同一套手法。
 */

/** 入口图形的渲染表；`registry` 只给身份，图形留在这里。 */
const ARTWORK: Record<GuideArtworkId, React.FC<{ size?: number; className?: string }>> = {
  files: GuideArtworkFiles,
  terminal: GuideArtworkTerminal,
};

/*
 * 入口卡片：372px、最小 56px、12px 圆角、14/20 内边距，白底加发丝线与浮层柔光。
 * 宽度夹在 360-380 之间，因此换一台机器、换一种字体度量，卡片的比例都成立。
 */
const ENTRY =
  'w-[372px] max-w-full min-h-14 px-5 py-3.5 flex items-center gap-3.5 rounded-md bg-surface hairline border-border-hairline shadow-panel text-left transition-colors duration-100 hover:bg-hover';

/** 入口左侧的图形位：固定 24px，因此每一张卡片的文字都从同一条竖线开始。 */
const ENTRY_ICON = 'shrink-0 w-6 h-6 flex items-center justify-center text-text-secondary';

/** 快捷键提示：不画键帽，只把按键名按书写顺序用 `+` 连起来。 */
const ShortcutHint: React.FC<{ keys: string[] }> = ({ keys }) => (
  <span className="shrink-0 inline-flex items-center gap-[3px] text-[11px] leading-4 text-text-tertiary">
    {keys.map((key, index) => (
      <React.Fragment key={`${key}-${index}`}>
        {index > 0 && <kbd className="font-sans">+</kbd>}
        <kbd className="font-sans">{key}</kbd>
      </React.Fragment>
    ))}
  </span>
);

/** 新建终端入口上的 shell 选择：展开时才去问本机有哪些 shell。 */
const ShellMenu: React.FC<{ onPick: (path: string) => void }> = ({ onPick }) => {
  const [shells, setShells] = React.useState<ShellChoiceItem[] | null>(null);
  // 调试桥没有终端能力，问它只会得到「未知命令」；这里如实说明而不是谎报「本机没有 shell」。
  const supported = ipc.terminalSupported();

  const load = React.useCallback(() => {
    if (!supported) return;
    setShells(null);
    // 查询失败按「本机没有可选 shell」呈现：菜单里给一句说明，不弹错误。
    void ipc
      .terminalShells()
      .then(setShells)
      .catch(() => setShells([]));
  }, [supported]);

  /*
   * 展开回调必须是稳定的：浮层把它当成 effect 依赖，每次渲染都换一个新函数
   * 会让那个 effect 每个渲染都跑一遍，于是「展开就查一次」变成「一直查」。
   */
  const handleOpenChange = React.useCallback(
    (open: boolean) => {
      if (open) load();
    },
    [load],
  );

  return (
    <Dropdown
      align="start"
      className="pointer-events-auto relative z-10 shrink-0"
      onOpenChange={handleOpenChange}
      trigger={({ toggle }) => (
        <button
          type="button"
          aria-label="选择 shell"
          title="选择 shell"
          onClick={toggle}
          className="w-5 h-5 p-0 inline-flex items-center justify-center rounded-xs text-text-tertiary transition-colors duration-100 hover:bg-hover hover:text-text-primary"
        >
          <ChevronDownIcon size={14} />
        </button>
      )}
    >
      {!supported ? (
        <div className="px-2 py-1 text-meta leading-5 text-text-tertiary">
          终端需要在桌面应用中使用
        </div>
      ) : shells === null ? (
        <div className="px-2 py-1 text-meta leading-5 text-text-tertiary">读取中</div>
      ) : shells.length === 0 ? (
        <div className="px-2 py-1 text-meta leading-5 text-text-tertiary">
          本机没有找到可用的 shell
        </div>
      ) : (
        shells.map((shell) => (
          <button
            key={shell.path}
            type="button"
            role="menuitem"
            title={shell.path}
            onClick={() => onPick(shell.path)}
            className="w-full min-h-[30px] py-1 px-2 flex items-center gap-1.5 rounded-md text-left text-row leading-5 text-text-primary transition-colors duration-100 hover:bg-hover"
          >
            <span className="flex-1 min-w-0 truncate">{shell.name}</span>
          </button>
        ))
      )}
    </Dropdown>
  );
};

export interface GuideBodyProps {
  controller: RailController;
}

export const GuideBody: React.FC<GuideBodyProps> = ({ controller }) => {
  const entries = React.useMemo(() => guideEntries(), []);

  return (
    <div
      data-rail-guide
      className="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 px-6 bg-surface-sunken"
    >
      {/*
       * 罗盘是方位感，不是控件：44px，取中性灰里比水印再深一档的令牌，
       * 对下沉底约 1.5:1，看得见但不抢入口卡片。
       */}
      <span aria-hidden="true" className="flex mb-4 text-watermark-strong">
        <CompassIcon size={44} />
      </span>

      {entries.map((entry) => {
        const Artwork = ARTWORK[entry.artwork] ?? CubeIcon;
        const pick = () => controller.openPage(entry.kind, { replaceActive: true });
        if (entry.kind !== 'terminal') {
          return (
            <button
              key={entry.kind}
              type="button"
              data-rail-guide-entry={entry.kind}
              onClick={pick}
              className={ENTRY}
            >
              <span className={ENTRY_ICON}>
                <Artwork />
              </span>
              <span className="flex-1 min-w-0 flex flex-col gap-0.5">
                <span className="text-body leading-5 truncate text-text-primary">
                  {entry.title}
                </span>
                <span className="text-meta leading-5 truncate text-text-tertiary">
                  {entry.description}
                </span>
              </span>
              <ShortcutHint keys={entry.shortcut} />
            </button>
          );
        }
        /*
         * 终端入口多一个 shell 选择，因此它不能整卡是一个按钮：
         * 一张铺满卡片的透明按钮负责「就用默认 shell 开一个」，标题旁的箭头负责挑 shell。
         */
        return (
          <div key={entry.kind} data-rail-guide-entry={entry.kind} className={`${ENTRY} relative overflow-hidden`}>
            <button
              type="button"
              aria-label={`${entry.title} ${entry.description}`}
              onClick={pick}
              className="absolute inset-0 w-full h-full"
            />
            <span className={`${ENTRY_ICON} relative pointer-events-none`}>
              <Artwork />
            </span>
            <span className="relative flex-1 min-w-0 flex flex-col gap-0.5 pointer-events-none">
              <span className="flex items-center min-w-0 gap-1">
                <span className="text-body leading-5 truncate text-text-primary">
                  {entry.title}
                </span>
                <ShellMenu onPick={(path) => controller.newTerminal(path)} />
              </span>
              <span className="text-meta leading-5 truncate text-text-tertiary">
                {entry.description}
              </span>
            </span>
            <ShortcutHint keys={entry.shortcut} />
          </div>
        );
      })}

      <div aria-hidden="true" className="shrink basis-[10%]" />
    </div>
  );
};
