import React from 'react';
import { changedFilesOf } from '../../conversation/transcript';
import type { TurnState } from '../../conversation/transcript/state';
import { ChangedFilesCard } from './ChangedFilesCard';
import { TranscriptCellView } from './Transcript';

/*
 * 一轮的正文块。
 *
 * 这里是正文渲染的成本分水岭：**以轮次为 memo 边界**，一次增量帧只让真正变化的那一轮
 * 重建元素，其余轮次连元素都不重建、引用回调也不重挂载。没有这道边界时，每一帧都要为
 * 全部单元重建元素并重挂每一行的 ref，正文越长，单个增量帧的代价越大（实测能到每秒
 * 上百 MB 的分配）。既有工作台产品的会话正文同样是节点级边界，这里对齐的就是这一点。
 *
 * 过程行（思考 / 工具 / 改动 / 历史）连续出现时按一组排：组内 0 间距、组与组之间 10px。
 * 同一段工作因此读起来是一段连续的过程日志，而不是一堆等距的卡片。
 */

/** 属于「过程」的单元：同一组内相邻时不再加间距。 */
const PROCESS_ROLES = new Set(['reasoning', 'tool', 'fileChange', 'history']);

interface CellGroup {
  key: string;
  process: boolean;
  cells: TurnState['cells'];
}

/** 把单元按「是否过程行」切成连续的组；非过程单元各自成组，保持原有的轮内间距。 */
function groupCells(cells: TurnState['cells']): CellGroup[] {
  const groups: CellGroup[] = [];
  for (const cell of cells) {
    const process = PROCESS_ROLES.has(cell.role);
    const last = groups[groups.length - 1];
    if (process && last !== undefined && last.process) {
      last.cells.push(cell);
      continue;
    }
    groups.push({ key: `group:${cell.id}`, process, cells: [cell] });
  }
  return groups;
}

export interface TurnBlockProps {
  turn: TurnState;
  /** 轮次序号，从 1 开始；只用于改动卡片的分组标题。 */
  turnNumber: number;
  /** 这一轮被窗口折掉的前导单元数。 */
  hiddenCells: number;
  /** 登记轮次锚点；正文导航条据此定位与高亮。 */
  onAnchor: (turnId: string, node: HTMLElement | null) => void;
  /** 打开某一轮的改动对比，并落到第 index 个文件。 */
  onOpenChange: (turnId: string, turnNumber: number, index: number) => void;
  /**
   * 是否按历史段落做原生虚拟化。
   *
   * 只对滚动窗口之外的历史轮次成立：`content-visibility: auto` 让浏览器跳过
   * 视口外内容的绘制与布局，百轮会话因此仍然满帧；正在流式输出的尾部永远不参与。
   */
  memoized?: boolean;
}

export const TurnBlock = React.memo<TurnBlockProps>(
  ({ turn, turnNumber, hiddenCells, onAnchor, onOpenChange, memoized = false }) => {
    const files = changedFilesOf(turn.changes);
    const cells = hiddenCells > 0 ? turn.cells.slice(hiddenCells) : turn.cells;
    const tail = cells[cells.length - 1];
    const endsWithSummary = tail !== undefined && tail.role === 'summary';
    /*
     * 改动卡片插在收尾分割线之前：改动属于这一轮真的做过的工作，分割线负责收口这一轮，
     * 因此卡片不会被读成下一轮的开场白；没有收尾时卡片落在轮末。
     */
    const card =
      files.length === 0 ? null : (
        <ChangedFilesCard
          files={files}
          onOpen={(index) => onOpenChange(turn.id, turnNumber, index)}
        />
      );
    const groups = groupCells(cells);
    const nodeRef = React.useRef<HTMLDivElement>(null);

    /*
     * 把真实高度写进 `--block-h`。
     *
     * `contain-intrinsic-size: auto var(--block-h)` 里的 `auto` 会记住上一次渲染的尺寸，
     * 但首次进入虚拟化时还没有任何记忆，必须有一个可信的占位高度，否则滚动条滑块
     * 会先按默认值算比例再跳一次。这里在轮次转为历史时量一次，之后不再读。
     */
    React.useLayoutEffect(() => {
      if (!memoized) return;
      const node = nodeRef.current;
      if (node === null) return;
      const height = Math.round(node.getBoundingClientRect().height);
      if (height > 0) node.style.setProperty('--block-h', `${height}px`);
    }, [memoized]);

    return (
      <div
        data-turn-id={turn.id}
        ref={(node) => {
          nodeRef.current = node;
          onAnchor(turn.id, node);
        }}
        className={`flex flex-col gap-2.5 ${memoized ? 'message-block-memoized' : ''}`}
      >
        {groups.map((group, position) => (
          <React.Fragment key={group.key}>
            {card !== null &&
              endsWithSummary &&
              position === groups.length - 1 &&
              group.process === false &&
              card}
            <div
              className={
                group.process
                  ? `flex flex-col ${group.cells.length > 1 ? 'pl-2 hairline-l border-border' : ''}`
                  : 'flex flex-col gap-2.5'
              }
            >
              {group.cells.map((cell) => (
                <TranscriptCellView key={cell.id} cell={cell} />
              ))}
            </div>
          </React.Fragment>
        ))}
        {card !== null && !endsWithSummary ? card : null}
      </div>
    );
  },
);

TurnBlock.displayName = 'TurnBlock';
