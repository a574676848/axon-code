import React from 'react';
import { extensionOf } from './viewers';
import { BodyShell } from './parts';
import type { ViewerProps } from './contract';

/*
 * 表格看法：把分隔符文本摆成一张表。
 *
 * 只认 `csv` / `tsv`：这两种是**纯文本**，读一遍就能摆出来，不需要表格引擎。
 * `.xlsx` / `.xls` 是压缩包，本工作台没有内置引擎，因此不在这里冒充：它们走
 * 「暂时无法预览」的空态，那里至少还能用系统里的应用打开。
 *
 * 解析按 RFC 4180 的口径：双引号包裹的字段里可以有分隔符、换行与双写引号。
 */

/** 最多展示的行数：再多的部分截断并如实标注。 */
const MAX_ROWS = 2000;

/** 最多展示的列数。 */
const MAX_COLUMNS = 100;

/** 把分隔符文本切成单元格。 */
export function parseDelimited(source: string, delimiter: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = '';
  let quoted = false;
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    if (quoted) {
      if (character === '"') {
        if (source[index + 1] === '"') {
          field += '"';
          index += 1;
        } else {
          quoted = false;
        }
      } else {
        field += character;
      }
      continue;
    }
    if (character === '"' && field.length === 0) {
      quoted = true;
    } else if (character === delimiter) {
      row.push(field);
      field = '';
    } else if (character === '\n') {
      row.push(field);
      rows.push(row);
      row = [];
      field = '';
    } else if (character !== '\r') {
      field += character;
    }
  }
  if (field.length > 0 || row.length > 0) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

/** 表格：首行固定表头，其余行可滚动。 */
export const TableBody: React.FC<ViewerProps> = ({ path, content }) => {
  const cells = React.useMemo(() => {
    if (content.kind !== 'text') return [];
    return parseDelimited(content.text, extensionOf(path) === 'tsv' ? '\t' : ',');
  }, [content, path]);

  if (content.kind !== 'text') return null;
  const [header, ...body] = cells;
  const columns = Math.min(header?.length ?? 0, MAX_COLUMNS);
  const rows = body.slice(0, MAX_ROWS);

  return (
    <BodyShell label={`${cells.length} 行 × ${header?.length ?? 0} 列`}>
      {header === undefined ? (
        <p className="px-4 py-4 text-row text-text-tertiary">这份表格里没有可显示的内容。</p>
      ) : (
        <table className="border-separate border-spacing-0 text-code font-mono">
          <thead>
            <tr>
              {header.slice(0, columns).map((value, index) => (
                <th
                  key={index}
                  className={`sticky top-0 z-10 max-w-[320px] truncate bg-surface-sunken pr-2.5 py-1.5 text-left font-medium text-text-primary hairline-b border-border ${
                    index === 0 ? 'pl-4' : 'pl-2.5'
                  }`}
                >
                  {value}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, rowIndex) => (
              <tr key={rowIndex} className="transition-colors duration-100 hover:bg-hover">
                {Array.from({ length: columns }, (_, columnIndex) => (
                  <td
                    key={columnIndex}
                    title={row[columnIndex] ?? ''}
                    className={`max-w-[320px] truncate pr-2.5 py-1 align-top text-text-primary hairline-b border-border-hairline ${
                      columnIndex === 0 ? 'pl-4' : 'pl-2.5'
                    }`}
                  >
                    {row[columnIndex] ?? ''}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {(body.length > MAX_ROWS || (header?.length ?? 0) > MAX_COLUMNS) && (
        <div className="px-4 py-1.5 text-meta text-text-tertiary">
          只显示前 {MAX_ROWS} 行、前 {MAX_COLUMNS} 列。
        </div>
      )}
    </BodyShell>
  );
};
