import React from 'react';
import { Button, SegmentedControl } from './controls';
import { PAGE_GAP, pageNumbers } from './paginationMath';
import { Text } from './surfaces';

/*
 * 传统分页条：左侧读数、右侧每页条数与页码。
 *
 * 全站只有这一份实现：请求日志（游标分页）与定价规则（本地分页）共用它，
 * 因此两处的页码外观、禁用规则与「上一页 / 下一页」的措辞完全一致。
 *
 * `reachable` 是给游标分页用的：后端的游标只能向前推，更远的页要先翻过去才拿得到
 * 游标，所以那些页码显示为不可点并在原生提示里说明原因，而不是点了没有反应。
 */

export interface PaginationProps {
  /** 当前页码，从 1 起。 */
  page: number;
  /** 总页数。 */
  pageCount: number;
  /** 已经能翻到的最大页码；缺省表示所有页码都可直达。 */
  reachable?: number;
  pageSize: number;
  /** 每页条数档位；省略时不显示这一项。 */
  pageSizeOptions?: readonly number[];
  onPageChange: (page: number) => void;
  onPageSizeChange?: (size: number) => void;
  /** 左侧读数，例如「第 1 至 20 条，共 71 条」。 */
  summary: string;
  /** 取数中：所有页码与翻页按钮禁用，避免连点叠加请求。 */
  busy?: boolean;
  /** 无障碍名称。 */
  label: string;
}

export const Pagination: React.FC<PaginationProps> = ({
  page,
  pageCount,
  reachable,
  pageSize,
  pageSizeOptions,
  onPageChange,
  onPageSizeChange,
  summary,
  busy = false,
  label,
}) => {
  const limit = reachable ?? pageCount;
  return (
    <div className="shrink-0 px-3.5 py-2.5 flex items-center gap-3 flex-wrap bg-surface-sunken hairline-t border-border-hairline">
      <Text tone="tertiary" size="meta" className="min-w-0" mono>
        {summary}
      </Text>
      <div className="flex-1 min-w-0" />
      {pageSizeOptions !== undefined && onPageSizeChange !== undefined && (
        <span className="inline-flex items-center gap-1.5">
          <Text tone="tertiary" size="meta">
            每页
          </Text>
          <SegmentedControl
            label="每页条数"
            value={String(pageSize)}
            onChange={(value) => onPageSizeChange(Number(value))}
            options={pageSizeOptions.map((size) => ({
              value: String(size),
              label: String(size),
            }))}
          />
        </span>
      )}
      <nav className="flex items-center gap-1" aria-label={label}>
        <Button
          variant="outline"
          size="sm"
          disabled={page <= 1 || busy}
          onClick={() => onPageChange(page - 1)}
        >
          上一页
        </Button>
        {pageNumbers(page, pageCount).map((item, index) =>
          item === PAGE_GAP ? (
            <span key={`gap-${index}`} className="px-1">
              <Text tone="tertiary" size="meta" mono>
                …
              </Text>
            </span>
          ) : (
            <button
              key={item}
              type="button"
              aria-current={item === page ? 'page' : undefined}
              disabled={item > limit || busy}
              title={item > limit ? '需要先翻到前面的页才能到达这一页' : `第 ${item} 页`}
              onClick={() => onPageChange(item)}
              className={`min-w-7 h-7 px-2 rounded-sm text-row tabular-nums transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-40 ${
                item === page
                  ? 'bg-primary text-white'
                  : 'text-text-secondary hover:text-text-primary hover:bg-hover'
              }`}
            >
              {item}
            </button>
          ),
        )}
        <Button
          variant="outline"
          size="sm"
          disabled={page >= pageCount || page >= limit || busy}
          onClick={() => onPageChange(page + 1)}
        >
          下一页
        </Button>
      </nav>
    </div>
  );
};
