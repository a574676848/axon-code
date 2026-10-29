import React from 'react';
import { ChevronRightIcon, CodeIcon } from '../../icons';
import { changeTotals, type ChangedFile } from '../../conversation/transcript';
import { DiffStat, DisclosureBody, DisclosureRow } from './DisclosureRow';
import { FileTypeIcon } from './fileIcons';

/*
 * 本轮改动卡片：一轮结束时集中展示这一轮改了哪些文件。
 *
 * 事实源是订阅流上的 fileChange 事件归并出的改动集合，因此卡片里出现的每一个路径
 * 都是后端真的宣告过的改动，不做任何推断。
 *
 * 头部与计划卡同一几何：28px 行高、行首图标、「改动」标签、13px 标题与行尾 11px 增删读数，
 * 整行是折叠开关，默认收起。收起态只报「改了多少」，明细按需展开；展开后列出全部文件，
 * 点任一行在右侧栏打开本轮改动对比并落到该文件。
 */

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
  const [open, setOpen] = React.useState(false);
  const single = files.length === 1 ? files[0] : null;
  const totals = changeTotals(files);
  const title = single === null ? `已编辑 ${files.length} 个文件` : `已编辑 ${baseName(single.path)}`;

  return (
    <div
      className="w-full rounded-md hairline border-border shadow-card bg-surface px-1.5 py-0.5"
      data-changed-files
      data-changed-files-count={files.length}
    >
      <DisclosureRow
        open={open}
        onToggle={() => setOpen((value) => !value)}
        icon={
          single === null ? <CodeIcon size={14} /> : <FileTypeIcon path={single.path} size={14} />
        }
        label="改动"
        title={title}
        titleText={single === null ? undefined : single.path}
        meta={<DiffStat added={totals.added} removed={totals.removed} />}
      />

      {open && (
        <DisclosureBody>
          <ul className="pr-1 space-y-0.5">
            {files.map((file, index) => (
              <li key={file.path}>
                <button
                  type="button"
                  onClick={() => onOpen(index)}
                  title={file.path}
                  aria-label={`查看 ${file.path} 的改动`}
                  className="group/file w-full h-7 px-1 flex items-center gap-1.5 rounded-xs text-left transition-colors duration-100 hover:bg-hover"
                >
                  <span className="flex-1 min-w-0 truncate text-row text-text-prose">
                    {file.path}
                    {file.movePath !== null && (
                      <span className="text-text-tertiary"> → {file.movePath}</span>
                    )}
                  </span>
                  <DiffStat added={file.added} removed={file.removed} className="shrink-0" />
                  <ChevronRightIcon
                    size={12}
                    className="shrink-0 text-text-tertiary opacity-0 transition-opacity duration-150 group-hover/file:opacity-100"
                  />
                </button>
              </li>
            ))}
          </ul>
        </DisclosureBody>
      )}
    </div>
  );
};
