import React from 'react';

import type { LinkToken } from '../../conversation/linkTokens';
import { splitComposerSegments } from '../../conversation/textSegments';

/*
 * 输入坞的输入框：一条真实 textarea 加一层垫在下面的高亮底衬。
 *
 * textarea 没法只给一部分文字上色，因此这里用完全相同的度量把同一段正文再画一层：
 * 同一字体与字号行高、同一内边距、同样的折行规则。textarea 自己的文字取透明，
 * 露出来的就是底衬里被标记的那几个片段，光标与选区仍然由真实输入框承担。
 *
 * 两处细节都不是可选的：
 * 1. 输入法组合期间把底衬藏起来、输入框文字转回正常色。组合中的文字带下划线，
 *    那是输入法自己画在 textarea 里的，文字透明会把它一起抹掉。
 * 2. 正文超过上限行数后 textarea 自己滚动，底衬跟着同步滚动，否则高亮与文字错位。
 *
 * 底衬按 textSegments 切分：已安装技能名的 `/name` 用技能蓝，粘贴落成的链接占位
 * 用链接蓝加下沉底色。两者都不加内边距，字位因此与真实输入框逐字对齐。
 */

interface ComposerTextAreaProps {
  /** 真实输入框节点：高度、选区与光标都由外部直接读写。 */
  areaRef: React.RefObject<HTMLTextAreaElement | null>;
  /** 正文。 */
  text: string;
  /** 是否可输入。 */
  disabled: boolean;
  placeholder: string;
  /** 已安装技能名：只有精确命中的 `/name` 才高亮。 */
  skillNames: readonly string[];
  /** 草稿里仍然看得见的链接占位：底衬按它把这几个占位画成链接。 */
  linkTokens: readonly LinkToken[];
  /** 单行高度，用作初始高度。 */
  height: number;
  /** 正文变化。 */
  onChange: (event: React.ChangeEvent<HTMLTextAreaElement>) => void;
  /** 光标位置变化（输入、点选、选区拖动都要报）。 */
  onCaret: (position: number) => void;
  onPaste: (event: React.ClipboardEvent<HTMLTextAreaElement>) => void;
  onCompositionStart: () => void;
  onCompositionEnd: (event: React.CompositionEvent<HTMLTextAreaElement>) => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  onContextMenu: (event: React.MouseEvent<HTMLTextAreaElement>) => void;
  /** 候选浮层的无障碍状态，直接透传给输入框。 */
  ariaExpanded: boolean;
  ariaControls?: string;
  ariaActiveDescendant?: string;
}

export const ComposerTextArea: React.FC<ComposerTextAreaProps> = ({
  areaRef,
  text,
  disabled,
  placeholder,
  skillNames,
  linkTokens,
  height,
  onChange,
  onCaret,
  onPaste,
  onCompositionStart,
  onCompositionEnd,
  onKeyDown,
  onContextMenu,
  ariaExpanded,
  ariaControls,
  ariaActiveDescendant,
}) => {
  const mirrorRef = React.useRef<HTMLDivElement>(null);
  /** 输入法组合中：底衬让位，文字交回输入框自己画。 */
  const [composing, setComposing] = React.useState(false);
  const segments = React.useMemo(
    () => splitComposerSegments(text, skillNames, linkTokens),
    [linkTokens, skillNames, text],
  );

  /**
   * 光标随文字一起报出来。
   *
   * 只在 select 事件里读光标是不够的：有些输入路径（程序化插入文本）只派发 input，
   * 不派发 select，那样光标会停在旧位置上，`/` 与 `@` 的判定跟着一起偏。这里在
   * 每一次变更与每一次点选里都读一次真实选区，光标因此始终与正文同源。
   */
  const reportCaret = (area: HTMLTextAreaElement) => onCaret(area.selectionStart ?? 0);

  return (
    <div className="relative">
      {/*
        底衬不接收指针、不进无障碍树：它只是同一段文字的另一次绘制。
        滚动同步写在 textarea 的 onScroll 上（元素自己的滚动，不是全局滚动监听）。
      */}
      <div
        ref={mirrorRef}
        aria-hidden="true"
        data-composer-mirror=""
        style={{ visibility: composing ? 'hidden' : 'visible' }}
        className="pointer-events-none absolute inset-0 overflow-hidden px-4 pt-[10px] pb-[10px] text-input whitespace-pre-wrap break-words"
      >
        {segments.map((segment, index) =>
          segment.kind === 'link' ? (
            // 占位符只换底色，不加内边距：字位与光标必须与真实输入框逐字对齐。
            <span key={index} className="rounded-xs bg-surface-sunken text-text-link">
              {segment.text}
            </span>
          ) : segment.kind === 'skill' ? (
            <span key={index} className="text-skill-ink">
              {segment.text}
            </span>
          ) : (
            <React.Fragment key={index}>{segment.text}</React.Fragment>
          ),
        )}
      </div>
      <textarea
        ref={areaRef}
        rows={1}
        value={text}
        disabled={disabled}
        placeholder={placeholder}
        onChange={(event) => {
          reportCaret(event.currentTarget);
          onChange(event);
        }}
        onPaste={onPaste}
        onCompositionStart={() => {
          setComposing(true);
          onCompositionStart();
        }}
        onCompositionEnd={(event) => {
          setComposing(false);
          onCompositionEnd(event);
        }}
        onSelect={(event) => reportCaret(event.currentTarget)}
        onClick={(event) => reportCaret(event.currentTarget)}
        onScroll={(event) => {
          const mirror = mirrorRef.current;
          if (mirror !== null) mirror.scrollTop = event.currentTarget.scrollTop;
        }}
        onKeyDown={onKeyDown}
        onContextMenu={onContextMenu}
        aria-expanded={ariaExpanded}
        aria-controls={ariaControls}
        aria-activedescendant={ariaActiveDescendant}
        /*
         * 高度只给下限：占位文案或正文折行时输入框自己长高，
         * 不会被一个固定高度裁成半行（窄列下曾出现「占位两行、只露一行半」）。
         *
         * 撑高由 CSS 的 `field-sizing: content` 负责（见 `.composer-input`），
         * 上限 6 行后再由输入框自己滚动：全程不读 `scrollHeight`，
         * 因此打字换行不付强制同步回流的代价。
         */
        style={{ minHeight: height }}
        className={`composer-input relative w-full resize-none bg-transparent px-4 py-2.5 text-input placeholder:text-text-placeholder outline-none disabled:cursor-not-allowed caret-text-primary ${
          composing ? 'text-text-primary' : 'text-transparent'
        }`}
      />
    </div>
  );
};
