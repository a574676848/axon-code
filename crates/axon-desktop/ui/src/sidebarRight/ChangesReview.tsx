import React from 'react';
import { CHANGE_KIND_LABEL, type ChangedFile } from '../conversation/transcript';
import { MAX_DIFF_ROWS, parseDiff, type DiffRow } from '../conversation/diffRows';
import { CheckIcon, ChevronDownIcon, CopyIcon, FileIcon, NoWrapIcon, WrapIcon } from '../icons';
import { Dropdown, MenuItem, MenuLabel } from '../components/ui/overlays';
import { EmptyState, Text } from '../components/ui/surfaces';
import { OpenInAppAction } from './OpenInAppAction';
import * as ipc from '../ipc';

/*
 * 本轮改动对比：右侧栏里一轮改动一个 tab，逐文件看这一轮开始与结束时的差别。
 *
 * 只读，事实源是订阅流上的改动集合，不向工作区另取一份内容：对比正文来自后端随
 * 改动事件给出的 diff 原文，因此即便文件此后又被改过，这里看到的仍是这一轮的事实。
 * 文件多于一个时由工具栏的选择器切换，行号取 hunk 头给的起始位置，逐行推下去。
 */

/** 复制成功的回执停留时长。 */
const COPIED_FEEDBACK_MS = 1500;

/** 行号列宽：四位足以覆盖万行以内的文件，超出由内容自己横向滚动。 */
const LINE_NUMBER_CLASS = 'shrink-0 select-none w-[4ch] text-right tabular-nums text-text-secondary';

/** 工具栏上的图标按钮。 */
const ToolButton: React.FC<{
  label: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}> = ({ label, onClick, disabled = false, children }) => (
  <button
    type="button"
    aria-label={label}
    title={label}
    disabled={disabled}
    onClick={onClick}
    className="w-7 h-7 inline-flex items-center justify-center rounded-sm text-text-tertiary transition-colors duration-100 hover:bg-hover hover:text-text-primary disabled:cursor-not-allowed disabled:text-text-caption disabled:hover:bg-transparent"
  >
    {children}
  </button>
);

/** 一行对比：旧行号、新行号、符号与正文；符号是配色的文本备份，色盲用户也读得出来。 */
const DiffLine: React.FC<{ row: DiffRow; wrap: boolean }> = ({ row, wrap }) => (
  <div
    className={`flex min-w-full ${
      row.kind === 'add' ? 'bg-diff-add' : row.kind === 'delete' ? 'bg-diff-delete' : ''
    }`}
  >
    <span className={`${LINE_NUMBER_CLASS} pl-2 pr-1.5`}>{row.oldLine ?? ''}</span>
    <span className={`${LINE_NUMBER_CLASS} pr-2`}>{row.newLine ?? ''}</span>
    <span className="shrink-0 select-none w-[2ch] text-text-secondary" aria-hidden="true">
      {row.kind === 'add' ? '+' : row.kind === 'delete' ? '-' : ' '}
    </span>
    <span className={wrap ? 'flex-1 min-w-0 whitespace-pre-wrap break-words pr-4' : 'whitespace-pre pr-4'}>
      {row.text}
    </span>
  </div>
);

export interface ChangesReviewProps {
  /** 工作区根目录绝对路径。 */
  root: string | null;
  /** 本轮的改动文件；null 表示这一轮的改动事实不在当前投影里。 */
  files: ChangedFile[] | null;
  /** 打开时落到的文件下标。 */
  index: number;
  /** 在右侧栏打开整份文件。 */
  onOpenFile: (path: string) => void;
}

export const ChangesReview: React.FC<ChangesReviewProps> = ({ root, files, index, onOpenFile }) => {
  const [selected, setSelected] = React.useState(index);
  const [wrap, setWrap] = React.useState(false);
  const [copied, setCopied] = React.useState(false);
  const [notice, setNotice] = React.useState<string | null>(null);

  // 同一个 tab 被再次导航到某个文件时，落点以最新一次导航为准。
  React.useEffect(() => {
    setSelected(index);
  }, [index]);

  React.useEffect(() => {
    if (!copied) return undefined;
    const timer = window.setTimeout(() => setCopied(false), COPIED_FEEDBACK_MS);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const file = files === null ? undefined : (files[selected] ?? files[0]);
  const sections = React.useMemo(
    () =>
      file === undefined
        ? []
        : file.segments.map((segment) => ({ segment, parsed: parseDiff(segment.kind, segment.text) })),
    [file],
  );

  const copyDiff = React.useCallback(() => {
    const text = file === undefined ? '' : file.segments.map((segment) => segment.text).join('\n');
    // 复制走原生剪贴板：网页剪贴板要求窗口处于焦点，复制会时灵时不灵。
    void ipc
      .clipboardWriteText(text)
      .then(() => setCopied(true))
      .catch(() => setNotice('剪贴板不可用，复制失败'));
  }, [file]);

  if (files === null || file === undefined) {
    return (
      <EmptyState
        className="flex-1"
        icon={<FileIcon size={20} />}
        title="这轮改动已不可用"
        description="改动事实来自这一轮的订阅流，会话重新打开后可能已经不在当前投影里。"
      />
    );
  }

  const path = file.path;
  const label = CHANGE_KIND_LABEL[file.kind];
  const counts = (
    <span className="shrink-0 font-mono text-code tabular-nums">
      <span className="text-ok-ink">+{file.added}</span>{' '}
      <span className="text-danger">-{file.removed}</span>
    </span>
  );

  return (
    <div className="min-h-0 flex-1 flex flex-col" data-changes-review data-changes-file={path}>
      <div className="shrink-0 h-9 pl-3 pr-2 flex items-center gap-1.5 hairline-b border-border-hairline bg-surface">
        {files.length === 1 ? (
          <span className="min-w-0 flex-1 truncate text-meta text-text-secondary" title={path}>
            {path}
          </span>
        ) : (
          <Dropdown
            align="start"
            className="flex-1 min-w-0"
            menuClassName="min-w-[280px] max-w-[420px]"
            trigger={({ toggle }) => (
              <button
                type="button"
                aria-haspopup="menu"
                aria-label="选择要查看的文件"
                title={path}
                onClick={toggle}
                className="w-full min-w-0 h-7 px-1.5 -ml-1 flex items-center gap-1 rounded-sm text-left transition-colors duration-100 hover:bg-hover"
              >
                <span className="min-w-0 flex-1 truncate text-meta text-text-primary">{path}</span>
                <ChevronDownIcon size={12} className="shrink-0 text-text-tertiary" />
              </button>
            )}
          >
            <MenuLabel>本轮改动的文件</MenuLabel>
            {files.map((entry, position) => (
              <MenuItem
                key={entry.path}
                selected={position === selected}
                onSelect={() => setSelected(position)}
              >
                <span className="w-full min-w-0 flex items-center gap-3">
                  <span className="min-w-0 flex-1 truncate">{entry.path}</span>
                  <span className="shrink-0 font-mono text-code tabular-nums">
                    <span className="text-ok-ink">+{entry.added}</span>{' '}
                    <span className="text-danger">-{entry.removed}</span>
                  </span>
                </span>
              </MenuItem>
            ))}
          </Dropdown>
        )}
        <span className="shrink-0 text-meta text-text-tertiary">{label}</span>
        {counts}
        <div className="shrink-0 flex items-center gap-0.5">
          <ToolButton label={wrap ? '取消换行' : '自动换行'} onClick={() => setWrap((value) => !value)}>
            {wrap ? <NoWrapIcon size={14} /> : <WrapIcon size={14} />}
          </ToolButton>
          <ToolButton label="在侧栏打开整份文件" onClick={() => onOpenFile(path)}>
            <FileIcon size={14} />
          </ToolButton>
          {root !== null && (
            <OpenInAppAction root={root} path={path} onNotice={setNotice} />
          )}
          <ToolButton label="复制改动" onClick={copyDiff}>
            {copied ? <CheckIcon size={14} className="text-ok-ink confirm-pop" /> : <CopyIcon size={14} />}
          </ToolButton>
        </div>
      </div>

      {notice !== null && (
        <div className="shrink-0 flex items-center gap-2.5 px-4 py-1.5">
          <Text tone="danger" size="meta" className="break-words">
            {notice}
          </Text>
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto scroll-visible font-mono text-code text-text-prose">
        {/*
         * 非换行模式下使用 min-w-full w-max 包裹整张对比表：
         * 保证横向滚动查看超长代码行时，新增/删除/hunk 底色始终铺满最长行的完整宽度，绝不半途截断。
         * hunk 头使用 whitespace-nowrap py-1 min-h-6，防止窄分栏下折行溢出压住首行代码。
         */}
        <div className={wrap ? 'w-full' : 'min-w-full w-max'}>
          {sections.map(({ segment, parsed }, position) => (
            <React.Fragment key={segment.partId}>
              {sections.length > 1 && (
                <div className="min-h-6 py-1 px-3 mt-2 first:mt-0 flex items-center min-w-full whitespace-nowrap bg-surface-sunken hairline-t border-border-hairline font-sans font-medium text-meta text-text-secondary">
                  {CHANGE_KIND_LABEL[segment.kind]} · 第 {position + 1} 次
                </div>
              )}
              {parsed.hunks.map((hunk, hunkIndex) => (
                <React.Fragment key={hunkIndex}>
                  {hunk.header !== null && (
                    <div
                      className={`min-h-6 py-1 px-3 flex items-center min-w-full bg-surface-sunken/75 hairline-y border-border-hairline text-meta text-text-tertiary ${
                        wrap ? 'whitespace-pre-wrap break-all' : 'whitespace-nowrap'
                      }`}
                    >
                      {hunk.header}
                    </div>
                  )}
                  {hunk.rows.map((row, rowIndex) => (
                    <DiffLine key={rowIndex} row={row} wrap={wrap} />
                  ))}
                </React.Fragment>
              ))}
              {parsed.truncated && (
                <div className="px-3 py-1.5 text-meta text-text-tertiary">
                  只显示前 {MAX_DIFF_ROWS} 行
                </div>
              )}
            </React.Fragment>
          ))}
        </div>
      </div>
    </div>
  );
};
