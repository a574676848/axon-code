import React from 'react';
import { ChevronRightIcon, StopIcon } from '../../icons';
import { formatDurationUnits } from '../../conversation/format';
import { jobElapsedMs, type JobRow } from '../../conversation/transcript';
import { isActiveJob } from '../../conversation/runtimeFacts';
import type { SessionJobFact } from '../../types';
import { StateDot } from '../ui/surfaces';
import { DisclosureRow } from './DisclosureRow';
import { ABNORMAL_TERMINAL, jobDotState, jobRowConclusion } from './jobRowStatus';
import { useJobStream } from './useJobStream';
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
 * 在跑的行由开始时刻按秒起算，因此列表里的数字会自己走；跑完的行只认后端实测值，
 * 不受采样时刻影响。开始时刻是服务端记录这条工具调用的时间，
 * 不是客户端第一次看到它的时间。判定本身在 `conversation/transcript` 里，便于单测。
 */
function elapsedOf(
  job: JobRow,
  running: boolean,
  nowMs: number,
  measuredMs: number | null,
): number | null {
  return jobElapsedMs(job, running, nowMs, measuredMs);
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
    className={`shrink-0 self-center h-6 inline-flex items-center justify-center rounded-sm hairline transition-colors duration-100 active:scale-95 disabled:cursor-not-allowed disabled:opacity-40 ${
      armed
        ? 'confirm-pop gap-1 px-1.5 border-danger bg-danger-soft text-danger font-medium'
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
const JobTerminal: React.FC<{
  job: JobRow;
  running: boolean;
  sessionId: string | null;
  /** 收口事实给出的退出码；没有事实时退回行内快照。 */
  exitCode: number | null;
  /** 收口事实给出的终态。 */
  status: string;
}> = ({ job, running, sessionId, exitCode, status }) => {
  const [copied, setCopied] = React.useState(false);
  const timer = React.useRef<number | null>(null);
  // 输出只认作业自己的归档：发起作业的那条工具回执（`job start …`）不是作业输出。
  const stream = useJobStream(sessionId, job.jobId, running);
  const outputRef = React.useRef<HTMLPreElement | null>(null);
  /** 是否贴在底部：贴住就跟随新内容，用户上翻阅读时不抢滚动位置。 */
  const stickRef = React.useRef(true);

  React.useEffect(() => {
    const node = outputRef.current;
    if (node === null || !stickRef.current) return;
    node.scrollTop = node.scrollHeight;
  }, [stream.text]);

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

  // 未跑完时不谈退出码；跑完且非零退出才给结论，没有证据就不下结论。
  // 判据与状态点的错误档同源（`jobRowStatus` 的 `ABNORMAL_TERMINAL`），两处不会分叉。
  const exitPill =
    running || exitCode === 0
      ? null
      : exitCode !== null
        ? `退出码 ${exitCode}`
        : ABNORMAL_TERMINAL.has(status)
          ? '未正常退出'
          : null;
  // 多行命令一行一个提示符：读起来就是它要跑的那几条命令。
  const body = job.command.endsWith('\n') ? job.command.slice(0, -1) : job.command;
  const lines = body.split('\n');
  const output = stream.text;
  const tailRunning = stream.running;
  const waitingFirstByte = tailRunning && output.length === 0 && stream.unavailable === null;
  // 识别命令里真正的输出落盘写法：PowerShell 的 Out-File/Set-Content/Add-Content，
  // 以及把 stdout 导向文件的重定向符。管道 `|` 不是重定向，`2>&1` 也不是：
  // 前者只是交接给下一个命令，后者是流合并，两者都不会让终端静默。
  const hasFileRedirection =
    /\b(?:out-file|set-content|add-content)\b|(?:^|[^0-9>\-])>{1,2}\s*[^&|;\r\n]*\.[a-z0-9]+/i.test(job.command);

  return (
    <div className="disclosure-enter mx-1 mb-1 rounded-md hairline border-border bg-code-block overflow-hidden">
      <div
        className={`relative flex items-start gap-3 pl-3 pr-3.5 py-2 ${COMMAND_CAP} overflow-auto scroll-visible hairline-b border-border-hairline`}
      >
        <div className="flex-1 min-w-0 flex flex-col font-mono text-code">
          {lines.map((line, index) => (
            <div key={index} className="flex items-baseline gap-2 min-w-0">
              <span className="shrink-0 text-text-tertiary">$</span>
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
          className={`shrink-0 sticky top-0 bg-code-block text-meta transition-colors duration-100 hover:text-text-primary ${
            copied ? 'text-ok-ink font-medium confirm-pop' : 'text-text-secondary'
          }`}
        >
          {copied ? '已复制' : '复制'}
        </button>
        {running && (
          <span
            aria-hidden="true"
            className="waiting-heartbeat-bar pointer-events-none absolute bottom-0 left-0 h-[1.5px] w-1/3 bg-text-link/55"
          />
        )}
      </div>
      {waitingFirstByte ? (
        <div className="px-3 py-2.5 flex items-center gap-2 text-meta text-text-tertiary">
          <StateDot state="ongoing" />
          <span className="shimmer-text-active">
            {hasFileRedirection
              ? '正在后台执行（命令包含输出重定向，实时标准输出可能被写入文件）...'
              : '正在后台执行，等待输出流...'}
          </span>
        </div>
      ) : output.length > 0 || tailRunning ? (
        <>
          <pre
            ref={outputRef}
            onScroll={(event) => {
              const node = event.currentTarget;
              stickRef.current = node.scrollHeight - node.scrollTop - node.clientHeight < 16;
            }}
            className={`${OUTPUT_CAP} pl-3 pr-3.5 py-3 overflow-auto scroll-visible font-mono text-code text-text-prose whitespace-pre-wrap break-words`}
          >
            {output}
          </pre>
          {stream.unavailable !== null && (
            <div className="px-3 pb-2 font-mono text-code text-text-tertiary">
              {stream.unavailable}
            </div>
          )}
        </>
      ) : (
        <div className="pl-3 pr-3.5 py-3 font-mono text-code text-text-tertiary">
          {stream.unavailable ?? '（无输出）'}
        </div>
      )}
    </div>
  );
};

export interface JobLineProps {
  job: JobRow;
  /** 所属车道：作业输出按车道与作业标识取。 */
  sessionId: string | null;
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
  /**
   * 后端给出的收口事实（真实耗时、终态与退出码）。
   *
   * 启动快照只说明「作业被发起」，作业自己的结局由它回答；没有这份事实的行
   * 一律不猜：不画耗时，也不假装自己知道它怎么收口的。
   */
  fact: SessionJobFact | null;
  onToggle: () => void;
  onStop: (job: JobRow) => void;
}

/** 作业一行：状态点打头，行尾给状态与耗时，可展开出终端卡。 */
export const JobLine: React.FC<JobLineProps> = ({
  job,
  sessionId,
  stoppedReason,
  live,
  expanded,
  armed,
  stopping,
  nowMs,
  fact,
  onToggle,
  onStop,
}) => {
  const running = live && isActiveJob(job.status) && stoppedReason === null;
  // 认得出作业的行都能展开看输出：在跑的行随时开始输出，跑完的行归档仍可读。
  const observable = running || job.output.length > 0 || job.jobId !== null;
  /*
   * 在跑的作业还没有结局：收口事实（可能来自上一代同名作业，作业标识在进程重启后
   * 从 1 重计）一律不套到这一行上，否则会出现「转圈 + 红色 exit N」的自相矛盾。
   */
  const settled = running ? null : fact;
  // 收口事实给出的终态优先于启动快照：它才是这条作业真实的结局。
  const effectiveStatus = settled?.status ?? job.status;
  const effectiveExitCode = settled === null ? job.exitCode : (settled.exitCode ?? null);
  // 行尾结论（文案 / 危险色 / 是否只是「后端已不再持有」）在 `jobRowStatus` 里判定，可被单测钉住。
  const conclusion = jobRowConclusion({
    status: effectiveStatus,
    exitCode: effectiveExitCode,
    live,
    stoppedReason,
  });
  const statusText = conclusion.text;
  const statusDanger = conclusion.danger;
  // 正常在跑的行不再重复「运行中」：转圈状态点与分组标题已经把这件事说清楚了。
  const liveDetail = statusText === '运行中' ? null : statusText;
  const duration = (
    <Duration
      durationMs={elapsedOf(job, running, nowMs, settled?.elapsedMs ?? null)}
      running={running}
    />
  );

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
            icon={<StateDot state={jobDotState(effectiveStatus, { running, stopped: stoppedReason !== null })} />}
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
      {expanded && observable && (
        <JobTerminal
          job={job}
          running={running}
          sessionId={sessionId}
          exitCode={effectiveExitCode}
          status={effectiveStatus}
        />
      )}
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
