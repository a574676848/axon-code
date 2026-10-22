import React from 'react';
import { ChevronRightIcon, StopIcon } from '../../icons';
import { formatDurationUnits } from '../../conversation/format';
import type { JobRow } from '../../conversation/transcript';
import { isActiveJob } from '../../conversation/runtimeFacts';
import { StateDot, type StateDotState } from '../ui/surfaces';
import { DisclosureRow } from './DisclosureRow';
import * as ipc from '../../ipc';

/*
 * 后台任务列表：分组标题、任务行与展开后的终端卡。
 *
 * 行与时间线上的过程行同一几何（28px、左侧「命令」标签列、行尾 12px 状态与耗时），
 * 展开入口沿用唯一的一套箭头约定：收起朝右、展开朝下。
 * 展开后的卡片是一条 `$ 命令` 横幅加可滚动的输出区，命令随时可复制。
 */

/** 复制回执的停留窗口；与文件预览工具栏同一口径。 */
const COPY_RECEIPT_MS = 1500;

/** 命令横幅的高度上限：长命令在这一区域里滚动，一个命令行一行。 */
const COMMAND_CAP = 'max-h-37';

/** 输出区的高度上限：超出后区域内部滚动，中间行不折叠。 */
const OUTPUT_CAP = 'max-h-72';

/** 状态文本的最大宽度：失败原因是产出方的原文，没有长度上限。 */
const STATUS_MAX = 'max-w-[60%]';

/** 作业状态到状态点的映射；表与参考实现的作业列表逐项一致。 */
function jobDotState(status: string): StateDotState {
  if (status === 'running') return 'ongoing';
  // 正在停止与被停同属「按人的意思收口」，与参考实现共用警示色。
  if (status === 'stopping' || status === 'killed') return 'warning';
  if (status === 'completed') return 'done';
  if (status === 'failed') return 'error';
  // 既没在跑也没结果的中间态：排队等待与未知状态都落在这一档。
  return 'idle';
}

/** 状态的展示文案。 */
function statusLabel(status: string): string {
  if (status === 'running') return '运行中';
  if (status === 'stopping') return '正在停止';
  if (status === 'completed') return '已完成';
  if (status === 'killed') return '已取消';
  if (status === 'failed') return '已失败';
  return status;
}

/** 行尾耗时：等宽数字，位置固定，不随状态文本长度跳动。 */
const Duration: React.FC<{ durationMs: number | null; running: boolean }> = ({
  durationMs,
  running,
}) =>
  durationMs === null ? null : (
    <span
      className="shrink-0 tabular-nums text-meta text-text-tertiary"
      title={`${running ? '已运行' : '耗时'} ${formatDurationUnits(durationMs)}`}
    >
      {formatDurationUnits(durationMs)}
    </span>
  );

/**
 * 这一行此刻该显示的耗时。
 *
 * 在跑的行由开始时刻按秒起算，因此列表里的数字会自己走；跑完的行用后端给的
 * 实测耗时，不受采样时刻影响。开始时刻是服务端记录这条工具调用的时间，
 * 不是客户端第一次看到它的时间。
 */
function elapsedOf(job: JobRow, running: boolean, nowMs: number): number | null {
  const start = job.startedAtMs;
  if (running && start !== null && Number.isFinite(start)) return Math.max(0, nowMs - start);
  return job.durationMs;
}

/** 停止按钮：首击武装成带文案的确认胶囊，窗口内再击一次才真的停。 */
const StopButton: React.FC<{ armed: boolean; pending: boolean; onStop: () => void }> = ({
  armed,
  pending,
  onStop,
}) => (
  <button
    type="button"
    disabled={pending}
    onClick={onStop}
    title={armed ? '再点一次即停止该作业' : '停止该作业需要再确认一次'}
    className={`shrink-0 self-center h-6 inline-flex items-center justify-center rounded-sm hairline transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-40 ${
      armed
        ? 'gap-1 px-1.5 border-danger bg-danger-soft text-danger'
        : 'w-6 border-border bg-surface text-text-secondary hover:border-danger hover:text-danger'
    }`}
  >
    <StopIcon size={12} />
    {armed && <span className="text-meta whitespace-nowrap">确认停止</span>}
  </button>
);

/**
 * 展开后的终端卡：命令横幅加输出区。
 *
 * 命令按原样换行铺满，横幅限高后自己滚动，因此再长的指令也读得全；退出码标签
 * 与复制入口贴住横幅顶端，命令滚走时它们还在。在跑且尚无输出的命令只有横幅，
 * 不画空的输出区；输出区同样限高内部滚动，中间行不折叠。
 */
const JobTerminal: React.FC<{ job: JobRow; running: boolean }> = ({ job, running }) => {
  const [copied, setCopied] = React.useState(false);
  const timer = React.useRef<number | null>(null);

  React.useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    [],
  );

  const copyCommand = () => {
    // 复制走原生剪贴板：网页剪贴板要求窗口处于焦点，复制会时灵时不灵。
    void ipc.clipboardWriteText(job.command).catch(() => undefined);
    setCopied(true);
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), COPY_RECEIPT_MS);
  };

  // 未跑完时不谈退出码；跑完且非零退出或没有退出码才给结论，干净退出不画标签。
  const exitPill =
    running || job.exitCode === 0
      ? null
      : job.exitCode === null
        ? '未正常退出'
        : `退出码 ${job.exitCode}`;
  // 多行命令一行一个提示符：读起来就是它要跑的那几条命令。
  const body = job.command.endsWith('\n') ? job.command.slice(0, -1) : job.command;
  const lines = body.split('\n');
  const hasBody = !running || job.output.length > 0;

  return (
    <div className="mx-1 mb-1 rounded-md bg-code-block overflow-hidden">
      <div
        className={`flex items-start gap-3 pl-3 pr-3.5 py-2 ${COMMAND_CAP} overflow-auto scroll-visible ${
          hasBody ? 'hairline-b border-border' : ''
        }`}
      >
        <div className="flex-1 min-w-0 flex flex-col font-mono text-code">
          {lines.map((line, index) => (
            <div key={index} className="flex items-baseline gap-2 min-w-0">
              {/* 工作目录只有一条，因此只有首行谈它，其余行只用 `$` 对齐。 */}
              <span className="shrink-0 text-text-tertiary">$</span>
              {/*
               * 按原样换行铺满：正常命令在有空格处折行（与参考实现同款），
               * 而超长的不可断行片段（长路径、长参数）不强行折断，改为让横幅横向滚动，
               * 因此指令既读得全，也随时能滚到手够不到的那一段。
               */}
              <span className="min-w-0 text-text-primary whitespace-pre-wrap">{line}</span>
            </div>
          ))}
        </div>
        {exitPill !== null && (
          <span className="shrink-0 sticky top-0 text-meta text-danger">{exitPill}</span>
        )}
        {job.jobId !== null && (
          <button
            type="button"
            title="复制作业 ID"
            onClick={() => void ipc.clipboardWriteText(job.jobId ?? '').catch(() => undefined)}
            className="shrink-0 sticky top-0 bg-code-block text-meta tabular-nums text-text-secondary transition-colors duration-100 hover:text-text-primary"
          >
            #{job.jobId}
          </button>
        )}
        <button
          type="button"
          onClick={copyCommand}
          className="shrink-0 sticky top-0 bg-code-block text-meta text-text-secondary transition-colors duration-100 hover:text-text-primary"
        >
          {copied ? '已复制' : '复制'}
        </button>
      </div>
      {hasBody &&
        (job.output.length > 0 ? (
          <pre
            className={`${OUTPUT_CAP} pl-3 pr-3.5 py-3 overflow-auto scroll-visible font-mono text-code text-text-primary whitespace-pre-wrap break-words`}
          >
            {job.output}
          </pre>
        ) : (
          <div className="pl-3 pr-3.5 py-3 font-mono text-code text-text-tertiary">（无输出）</div>
        ))}
    </div>
  );
};

export interface JobLineProps {
  job: JobRow;
  /** 已被用户停止时的终态原因；为空表示未停止。 */
  stoppedReason: string | null;
  /** 后端是否仍持有它；false 表示这一行只是历史记录。 */
  live: boolean;
  /** 是否展开输出面板。 */
  expanded: boolean;
  /** 是否已武装（等待第二次点击确认）。 */
  armed: boolean;
  /** 是否正在停止。 */
  stopping: boolean;
  /** 本秒的采样时刻；在跑的行据此走高。 */
  nowMs: number;
  onToggle: () => void;
  onStop: (job: JobRow) => void;
}

/** 作业一行：状态点打头，行尾给状态与耗时，可展开出终端卡。 */
export const JobLine: React.FC<JobLineProps> = ({
  job,
  stoppedReason,
  live,
  expanded,
  armed,
  stopping,
  nowMs,
  onToggle,
  onStop,
}) => {
  const running = live && isActiveJob(job.status) && stoppedReason === null;
  // 有输出的行才可展开；在跑的行随时可能开始输出，因此一律可展开。
  const observable = running || job.output.length > 0;
  // 行里仍声称在跑、后端却已不再持有：如实说明，不假装它还在跑。
  const stale = !live && isActiveJob(job.status);
  /** 行内状态：停止原因优先，其次是退出码，最后才是状态词。 */
  const statusText =
    stoppedReason ??
    (job.exitCode !== null
      ? `exit ${job.exitCode}`
      : stale
        ? '已结束（后端已不再持有）'
        : statusLabel(job.status));
  const statusDanger =
    stoppedReason !== null ||
    (job.exitCode !== null && job.exitCode !== 0) ||
    job.status === 'failed' ||
    job.status === 'killed';
  // 正常在跑的行不再重复「运行中」：转圈状态点与分组标题已经把这件事说清楚了。
  const liveDetail = statusText === '运行中' ? null : statusText;
  const duration = <Duration durationMs={elapsedOf(job, running, nowMs)} running={running} />;

  return (
    <div className="flex flex-col">
      {/*
       * 进行中的行是一张填充卡：在跑的活压过已结束的尾巴；卡的左右内边距同时
       * 框住行与行尾控件，悬浮底色落在卡上，控件不会掉出卡外。
       */}
      <div
        className={`flex items-stretch gap-1.5 ${
          running ? 'rounded-md bg-surface-sunken pl-0.5 pr-1' : ''
        }`}
      >
        <div className="flex-1 min-w-0">
          <DisclosureRow
            open={observable ? expanded : undefined}
            onToggle={observable ? onToggle : undefined}
            rowRole="menuitem"
            icon={<StateDot state={jobDotState(job.status)} />}
            label="命令"
            title={<span className="font-mono">{job.command}</span>}
            titleText={job.command}
            meta={
              <span className="inline-flex items-center gap-2">
                {liveDetail !== null && (
                  <span className={`truncate ${STATUS_MAX} ${statusDanger ? 'text-danger' : ''}`}>
                    {liveDetail}
                  </span>
                )}
                {duration}
              </span>
            }
          />
        </div>
        {running && job.jobId !== null && (
          <StopButton armed={armed} pending={stopping} onStop={() => onStop(job)} />
        )}
      </div>
      {expanded && observable && <JobTerminal job={job} running={running} />}
    </div>
  );
};

export interface SectionHeaderProps {
  label: string;
  /** 首个分组不画上边线。 */
  first: boolean;
  /** 有折叠入口时给出当前展开态与切换回调。 */
  fold?: { expanded: boolean; onToggle: () => void };
  action?: { label: string; onClick: () => void };
}

/** 分组标题：与上一组用发丝线分隔，首组不画线。 */
export const SectionHeader: React.FC<SectionHeaderProps> = ({ label, first, fold, action }) => (
  <div
    className={`flex items-center justify-between gap-2 mx-1 mt-1 mb-px pt-1 pb-0.5 text-meta text-text-tertiary ${
      first ? '' : 'hairline-t border-border-hairline'
    }`}
  >
    {fold === undefined ? (
      <span className="truncate">{label}</span>
    ) : (
      <button
        type="button"
        aria-expanded={fold.expanded}
        onClick={fold.onToggle}
        className="min-w-0 inline-flex items-center gap-1 rounded-sm px-1 py-0.5 transition-colors duration-100 hover:bg-hover hover:text-text-secondary"
      >
        <ChevronRightIcon
          size={12}
          className={`shrink-0 transition-transform duration-150 ease-out ${
            fold.expanded ? 'rotate-90' : ''
          }`}
        />
        <span className="truncate">{label}</span>
      </button>
    )}
    {action !== undefined && (
      <button
        type="button"
        onClick={action.onClick}
        className="shrink-0 rounded-sm px-1 py-0.5 transition-colors duration-100 hover:bg-hover hover:text-text-secondary"
      >
        {action.label}
      </button>
    )}
  </div>
);
