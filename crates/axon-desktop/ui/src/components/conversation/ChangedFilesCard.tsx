import React from 'react';
import { ChevronRightIcon, CodeIcon } from '../../icons';
import { changeTotals, type ChangedFile } from '../../conversation/transcript';
import { DiffStat, DisclosureRow } from './DisclosureRow';
import { FileTypeIcon } from './fileIcons';

/*
 * 本轮改动卡片：一轮结束时集中展示这一轮改了哪些文件。
 *
 * 事实源是订阅流上的 fileChange 事件归并出的改动集合，因此卡片里出现的每一个路径
 * 都是后端真的宣告过的改动，不做任何推断。行点击在右侧栏打开本轮改动对比并落到该文件，
 * 标题则落到第一个文件上。折叠到四行，超出部分由底部控件展开。
 *
 * 头部与时间线上的过程行同一几何：28px 行高、13px 标题、行尾 11px 增删读数。
 */

/** 折叠前显示的行数。 */
const COLLAPSED_ROWS = 4;

/**
 * 改动卡正文的缩进。
 *
 * 头部没有展开箭头，标签从图标后开始（2px + 14px 图标 + 6px = 22px），
 * 因此不能沿用过程行含箭头与标签列的 34px。16px + 2px 引导线 + 4px 内边距 = 22px，
 * 文件列表正好与「改动」标签共用同一条左轴。
 */
const BODY_INDENT = 'ml-[16px]';

/** 文件名；路径的目录部分不参与标题。 */
function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

export interface ChangedFilesCardProps {
  files: ChangedFile[];
  /** 在右侧栏打开本轮改动对比，并落到指定下标的文件。 */
  onOpen: (index: number) => void;
}

export const ChangedFilesCard: React.FC<ChangedFilesCardProps> = ({ files, onOpen }) => {
  const [expanded, setExpanded] = React.useState(false);
  const single = files.length === 1 ? files[0] : null;
  const foldable = files.length > COLLAPSED_ROWS;
  const rows = foldable && !expanded ? files.slice(0, COLLAPSED_ROWS) : files;
  const totals = changeTotals(files);
  const title = single === null ? `已编辑 ${files.length} 个文件` : `已编辑 ${baseName(single.path)}`;

  return (
    <div
      className="w-full rounded-md hairline border-border-hairline bg-surface px-1 py-0.5"
      data-changed-files
      data-changed-files-count={files.length}
    >
      <DisclosureRow
        className="hairline-b border-border-hairline"
        onToggle={() => onOpen(0)}
        icon={
          single === null ? <CodeIcon size={14} /> : <FileTypeIcon path={single.path} size={14} />
        }
        label="改动"
        title={title}
        titleText={single === null ? '在右侧栏查看本轮改动' : single.path}
        meta={<DiffStat added={totals.added} removed={totals.removed} />}
        trailing={<ChevronRightIcon size={12} className="shrink-0 text-text-tertiary" />}
      />

      {single === null && (
        <ul className={`${BODY_INDENT} mr-1 mb-1 border-l-[2px] border-border-hairline pl-1 divide-hairline`}>
          {rows.map((file, index) => (
            <li key={file.path}>
              <button
                type="button"
                onClick={() => onOpen(index)}
                title={file.path}
                aria-label={`查看 ${file.path} 的改动`}
                className="w-full h-7 flex items-center gap-2 rounded-sm text-left transition-colors duration-100 hover:bg-hover"
              >
                <span className="flex-1 min-w-0 truncate text-row text-text-primary">
                  {file.path}
                  {file.movePath !== null && (
                    <span className="text-text-tertiary"> → {file.movePath}</span>
                  )}
                </span>
                <DiffStat added={file.added} removed={file.removed} className="shrink-0" />
              </button>
            </li>
          ))}
        </ul>
      )}

      {foldable && (
        <div className={`${BODY_INDENT} mr-1 mb-1 border-l-[2px] border-border-hairline pl-1`}>
          <button
            type="button"
            onClick={() => setExpanded((value) => !value)}
            aria-expanded={expanded}
            aria-label={expanded ? '收起改动文件列表' : `展开全部 ${files.length} 个改动文件`}
            className="w-full h-7 pr-1 flex items-center gap-1 rounded-sm text-meta text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary"
          >
            <ChevronRightIcon
              size={12}
              className={`shrink-0 text-text-tertiary transition-transform duration-150 ease-out ${
                expanded ? 'rotate-90' : ''
              }`}
            />
            {expanded ? '收起' : `全部 ${files.length} 个文件`}
          </button>
        </div>
      )}
    </div>
  );
};
