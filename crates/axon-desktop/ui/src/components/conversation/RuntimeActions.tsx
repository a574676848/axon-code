import React from 'react';
import * as ipc from '../../ipc';
import { AgentIcon, ChevronDownIcon, ChevronRightIcon, TerminalIcon } from '../../icons';
import {
  formatBasisPointsPercent,
  formatDecimalCount,
  formatDurationPadded,
} from '../../conversation/format';
import type { JobRow, SubagentRow } from '../../conversation/transcript';
import { isActiveJob } from '../../conversation/runtimeFacts';
import { Dropdown } from '../ui/overlays';
import { StateDot, type StateDotState } from '../ui/surfaces';
import { JobLine, SectionHeader } from './JobPanel';

/*
 * 会话 header 里的运行时控件：子代理与后台任务。
 *
 * 状态属于「随会话走的过程信息」，不是常驻面板：两个入口只在真的存在对应事实时
 * 才出现，普通对话不会为没用到的能力长出控件。详情收进下拉层，需要细看时才展开。
 *
 * 两个列表的行骨架由各自的模块承担：子代理行在本文件，后台任务列表在 JobPanel，
 * 后者与既有工作台产品的作业列表同一套排版。
 */

/** 停止确认窗口：首击武装，窗口内再击一次才真的停。 */
const STOP_CONFIRM_MS = 3000;

/** 活作业名册的对账间隔；比会话轮询密一档，作业状态变化更频繁。 */
const JOB_ROSTER_REFRESH_MS = 10000;

/** 子代理会话状态的对账间隔。 */
const SUBAGENT_ROSTER_REFRESH_MS = 10000;

/** 行内状态到状态点的映射；表与参考实现的作业状态一一对应。 */
function statusDotState(status: string): StateDotState {
  if (
    status === 'running' ||
    status === 'starting' ||
    status === 'stopping' ||
    status === 'pending'
  ) {
    return 'ongoing';
  }
  if (status === 'completed') return 'done';
  if (status === 'failed') return 'error';
  // 被停、被杀与中断同属「按人的意思结束」，与参考实现共用警示色。
  if (status === 'killed' || status === 'interrupted' || status === 'stopped') return 'warning';
  // 既没在跑也没结果是中间态：排队等待与未知状态都落在这一档。
  return 'idle';
}

/** 子代理一行：状态图标打头，点开进入它的运行轨迹。 */
const SubagentLine: React.FC<{
  row: SubagentRow;
  /**
   * 后端对账后的真实状态。
   *
   * 主轮事件流里的生命周期事件在轮次结束后就不再产生，只靠它会让已结束的子代理
   * 永远停在运行中；对账到终态时以这里为准。
   */
  effectiveStatus: SubagentRow['status'];
  onOpen: () => void;
}> = ({ row, effectiveStatus, onOpen }) => {
  const active =
    effectiveStatus === 'running' || effectiveStatus === 'starting' || effectiveStatus === 'waiting';
  const statusLabel =
    effectiveStatus === 'starting'
      ? '启动中'
      : effectiveStatus === 'running'
        ? '运行中'
        : effectiveStatus === 'waiting'
          ? '等待中'
          : effectiveStatus === 'completed'
            ? '已完成'
            : effectiveStatus === 'interrupted'
              ? '已中断'
              : '执行失败';
  // 两行合计与参考实现的行高一致：标题一行、说明一行，都是 16px。
  const tokenMetric = row.usage === null ? null : `${formatDecimalCount(row.usage)} tok`;
  const durationMetric = row.elapsedMs > 0 ? formatDurationPadded(row.elapsedMs) : null;
  /*
   * 次行与参考实现同一套拼法：任务说明、状态，运行中再追加当前阶段。
   * 只有真正在跑的行才带阶段后缀：终态行再挂「生成中」会自相矛盾。
   */
  const secondary = [
    row.summary,
    statusLabel,
    active && row.activity.length > 0 ? row.activity : null,
  ]
    .filter((value): value is string => value !== null && value.length > 0)
    .join(' · ');
  /** 第三行：模型与缓存命中率；两项都缺时这一行不占位。 */
  const extra = [
    row.model,
    row.cacheHitRateBasisPoints === null
      ? null
      : `命中 ${formatBasisPointsPercent(row.cacheHitRateBasisPoints)}`,
  ].filter((value): value is string => value !== null && value.length > 0);
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onOpen}
      title={`查看 ${row.label} 的运行轨迹`}
      className="group w-full flex items-start gap-1.5 rounded-sm px-1.5 py-1.5 text-left text-meta transition-colors duration-100 hover:bg-hover"
    >
      <span className="shrink-0 w-3.5 inline-flex items-center justify-center self-start h-4">
        <StateDot state={statusDotState(effectiveStatus)} />
      </span>
      <span className="flex-1 min-w-0 flex flex-col">
        <span className="min-w-0 truncate text-row text-text-primary">{row.label}</span>
        <span className="min-w-0 truncate text-meta text-text-tertiary">
          {secondary}
        </span>
        {/*
         * 模型与缓存命中率不进指标列：它们不是右对齐的数字，跟在说明下面一行
         * 才读得成「这一行是谁在跑、命中多少」。
         */}
        {extra.length > 0 && (
          <span className="min-w-0 truncate text-meta tabular-nums text-text-tertiary">
            {extra.join(' · ')}
          </span>
        )}
      </span>
      {/*
       * 指标列与参考实现同构：令牌数在第一行、耗时在第二行，右对齐且等宽，
       * 因此数字变化不会带着标题一起抖。
       */}
      <span className="shrink-0 flex flex-col items-end text-meta text-text-tertiary tabular-nums whitespace-nowrap">
        <span>{tokenMetric ?? ''}</span>
        <span>{durationMetric ?? ''}</span>
      </span>
      <span className="shrink-0 w-6 h-6 inline-flex items-center justify-center rounded-sm text-text-tertiary transition-colors duration-100 group-hover:text-text-primary">
        <ChevronRightIcon size={14} />
      </span>
    </button>
  );
};

export interface RuntimeActionsProps {
  subagents: SubagentRow[];
  jobs: JobRow[];
  /** 当前会话车道；停止作业时作为归属参数。 */
  laneId: string | null;
  /** 打开某个子代理的运行轨迹。 */
  onOpenSubagent: (agentId: string) => void;
}

export const RuntimeActions: React.FC<RuntimeActionsProps> = ({
  subagents,
  jobs,
  laneId,
  onOpenSubagent,
}) => {
  /** 已被用户停止的作业：行标识到终态原因。 */
  const [stopped, setStopped] = React.useState<Record<string, string>>({});
  /** 已武装等待确认的作业：按**行标识**记，不按作业 ID，因为重启后 ID 会撞。 */
  const [armed, setArmed] = React.useState<string | null>(null);
  /** 正在停止的作业：同样按行标识。 */
  const [stopping, setStopping] = React.useState<string | null>(null);
  const [stopError, setStopError] = React.useState<string | null>(null);
  /** 被用户在本地清掉的已结束作业。 */
  const [dismissed, setDismissed] = React.useState<Record<string, true>>({});
  /** 展开输出面板的作业行；一次只展开一行，与参考实现一致。 */
  const [expandedId, setExpandedId] = React.useState<string | null>(null);
  /** 已结束分组的折叠态：用户点过就以点击为准，没点过则在有进行中作业时折起。 */
  const [settledOpen, setSettledOpen] = React.useState<boolean | null>(null);
  /** 作业列表是否展开；在跑的行只在列表看得见时才走秒。 */
  const [jobsOpen, setJobsOpen] = React.useState(false);
  /** 本秒的采样时刻：在跑的行按秒起算耗时，因此这个值每秒更新一次。 */
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  /**
   * 后端此刻真正持有的作业标识。
   *
   * `null` 表示还没对过账（例如刚打开面板）：此时不把任何行判成失效，
   * 免得网络慢一拍就把正在跑的作业显示成已结束。
   */
  const [liveJobIds, setLiveJobIds] = React.useState<Set<string> | null>(null);
  const armTimer = React.useRef<number | null>(null);

  // 作业行的对账：transcript 投影是历史事实，进程重启后旧行不会自己消失，
  // 因此定期向后端要一次活作业名册，不在这份名册里的行不再算「运行中」。
  React.useEffect(() => {
    if (laneId === null || jobs.length === 0) {
      setLiveJobIds(null);
      return undefined;
    }
    let cancelled = false;
    const refresh = () => {
      void ipc
        .conversationLaneJobs(laneId)
        .then((roster) => {
          if (!cancelled) setLiveJobIds(new Set(roster.jobIds));
        })
        .catch(() => {
          // 对账失败保持上一次结论：宁可短暂显示旧状态，也不要误判成已结束。
        });
    };
    refresh();
    const timer = window.setInterval(refresh, JOB_ROSTER_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [laneId, jobs.length]);

  /**
   * 同一个作业标识可能对应多行：作业 ID 在进程内单调，但**重启后会从 1 重新计数**，
   * 于是「上一代的死作业」和「这一代的活作业」可以撞同一个 ID。
   * 按出现顺序只认最后一行，旧行一律按失效处理，停止键因此永远指向活的那个。
   */
  const lastRowByJobId = React.useMemo(() => {
    const last = new Map<string, string>();
    for (const job of jobs) {
      if (job.jobId !== null) last.set(job.jobId, job.id);
    }
    return last;
  }, [jobs]);

  /** 这一行是否仍是后端持有的活作业。 */
  const isLive = React.useCallback(
    (job: JobRow): boolean => {
      if (job.jobId === null) return false;
      if (lastRowByJobId.get(job.jobId) !== job.id) return false;
      if (liveJobIds === null) return true;
      return liveJobIds.has(job.jobId);
    },
    [lastRowByJobId, liveJobIds],
  );

  React.useEffect(
    () => () => {
      if (armTimer.current !== null) window.clearTimeout(armTimer.current);
    },
    [],
  );

  const reasonOf = React.useCallback(
    (job: JobRow) => stopped[job.id] ?? null,
    [stopped],
  );

  /** 两击式停止：首击武装并起表，窗口内再击一次才发请求。 */
  const requestStop = React.useCallback(
    (job: JobRow) => {
      const jobId = job.jobId;
      if (jobId === null || laneId === null) return;
      if (armTimer.current !== null) window.clearTimeout(armTimer.current);
      if (armed !== job.id) {
        setStopError(null);
        setArmed(job.id);
        armTimer.current = window.setTimeout(() => setArmed(null), STOP_CONFIRM_MS);
        return;
      }
      setArmed(null);
      setStopping(job.id);
      void ipc
        .conversationStopJob(laneId, jobId)
        .then((receipt) => {
          setStopped((current) => ({ ...current, [job.id]: receipt.reason ?? '已停止' }));
        })
        .catch((failure: unknown) => {
          // IPC 拒绝既可能是 Error，也可能是宿主直接抛出的字符串；
          // 两种都要原样呈现，否则服务端给出的拒绝原因会被一句兜底文案吃掉。
          setStopError(
            failure instanceof Error
              ? failure.message
              : typeof failure === 'string' && failure.length > 0
                ? failure
                : '停止作业失败',
          );
        })
        .finally(() => setStopping(null));
    },
    [armed, laneId],
  );

  // 已结束的作业在有进行中作业时折叠到分组之后，避免长列表盖住当前进展。
  const active = React.useMemo(
    () => jobs.filter((job) => isActiveJob(job.status) && isLive(job) && reasonOf(job) === null),
    [jobs, isLive, reasonOf],
  );
  const finished = React.useMemo(
    () =>
      jobs.filter(
        (job) =>
          (!isActiveJob(job.status) || !isLive(job) || reasonOf(job) !== null) &&
          dismissed[job.id] !== true,
      ),
    [jobs, isLive, reasonOf, dismissed],
  );
  const finishedOpen = settledOpen ?? active.length === 0;

  /*
   * 走秒的时钟只服务于看得见的在跑作业：列表没开或没有在跑的作业时不起表，
   * 因此静止的界面上一秒一个的定时器也不存在。开表那一刻先取一次时刻，
   * 否则第一帧会拿挂载时的旧值，长任务会先显示一个偏小的耗时。
   */
  React.useEffect(() => {
    if (!jobsOpen || active.length === 0) return undefined;
    setNowMs(Date.now());
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [jobsOpen, active.length]);

  // 展开中的行离开列表（换了一批事实）时收起面板：不留一个没有行的展开态。
  React.useEffect(() => {
    if (expandedId === null || jobs.some((job) => job.id === expandedId)) return;
    setExpandedId(null);
  }, [jobs, expandedId]);

  const ordered = React.useMemo(
    () =>
      [...subagents].sort(
        (left, right) => right.lastSeenAtMs - left.lastSeenAtMs || left.order - right.order,
      ),
    [subagents],
  );

  /*
   * 子代理状态对账。
   *
   * 行里的状态来自主轮事件流：父轮结束后不会再有生命周期事件，只靠它会让已经
   * 结束或被中断的子代理一直显示「运行中」。后端线程存储才是「此刻是否还在跑」
   * 的可靠依据（turnStarted → active，turnCompleted/Interrupted/Failed → idle），
   * 所以这里定期取一次子代理会话清单，把仍在声称运行、但后端已判定收口的行纠正成终态。
   */
  const [liveSubagents, setLiveSubagents] = React.useState<Map<
    string,
    { status: string; lastTurnStatus: string | null }
  > | null>(null);

  React.useEffect(() => {
    if (laneId === null || subagents.length === 0) {
      setLiveSubagents(null);
      return undefined;
    }
    let cancelled = false;
    const refresh = () => {
      void ipc
        .conversationLaneSubagents(laneId)
        .then((list) => {
          if (cancelled) return;
          const map = new Map<string, { status: string; lastTurnStatus: string | null }>();
          for (const item of list.subagents) {
            map.set(item.slug, {
              status: item.status ?? 'notLoaded',
              lastTurnStatus: item.lastTurnStatus ?? null,
            });
          }
          setLiveSubagents(map);
        })
        .catch(() => {
          // 对账失败保留上一次结论：宁可短暂显示旧状态，也不要误判成已结束。
        });
    };
    refresh();
    const timer = window.setInterval(refresh, SUBAGENT_ROSTER_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [laneId, subagents.length]);

  /** 该子代理的会话 slug，规则与运行侧 `child_session_id` 一致。 */
  const slugOfAgent = React.useCallback(
    (agentId: string) => agentId.replace(/^subagent:/, '').replace(/^\/+/, '').replaceAll('/', '_'),
    [],
  );

  /** 后端对账后的真实状态；无法对账时沿用事件流里的状态。 */
  const statusOf = React.useCallback(
    (row: SubagentRow): SubagentRow['status'] => {
      const live = liveSubagents?.get(slugOfAgent(row.agentId));
      if (live === undefined || live === null) return row.status;
      // 后端说这条会话仍在跑，就以事件流为准（它可能已经有更新的阶段）。
      if (live.status === 'active') return row.status;
      // 后端已判定收口：行里若还声称在跑，按末轮状态纠正。
      const claimedRunning =
        row.status === 'running' || row.status === 'starting' || row.status === 'waiting';
      if (!claimedRunning) return row.status;
      if (live.lastTurnStatus === 'failed') return 'failed';
      if (live.lastTurnStatus === 'interrupted') return 'interrupted';
      return 'completed';
    },
    [liveSubagents, slugOfAgent],
  );

  /** 仍在运行中的子代理数：触发标签上的计数必须与行内状态同源。 */
  const runningCount = React.useMemo(
    () =>
      ordered.filter((row) => {
        const status = statusOf(row);
        return status === 'running' || status === 'starting' || status === 'waiting';
      }).length,
    [ordered, statusOf],
  );

  if (ordered.length === 0 && jobs.length === 0) return null;

  const triggerClass =
    'h-7 px-1.5 inline-flex items-center gap-1 rounded-xs text-meta text-text-secondary tabular-nums transition-colors duration-100 hover:bg-hover hover:text-text-primary';
  return (
    <>
      {ordered.length > 0 && (
        <Dropdown
          align="end"
          menuClassName="w-[336px] max-w-[min(400px,100vw-32px)] p-1 flex flex-col gap-px max-h-[min(560px,100vh-140px)] overflow-y-auto scroll-visible"
          trigger={({ open, toggle }) => (
            <button type="button" onClick={toggle} title="子代理" className={triggerClass}>
              <AgentIcon size={14} className="shrink-0" />
              <span>子代理</span>
              <span className="mx-1">{runningCount}</span>
              <ChevronDownIcon
                size={12}
                className={`shrink-0 transition-transform duration-150 ease-out ${open ? 'rotate-180' : ''}`}
              />
            </button>
          )}
        >
          {ordered.map((row) => (
            <SubagentLine
              key={row.agentId}
              row={row}
              effectiveStatus={statusOf(row)}
              onOpen={() => onOpenSubagent(row.agentId)}
            />
          ))}
        </Dropdown>
      )}

      {jobs.length > 0 && (
        <Dropdown
          align="end"
          menuClassName="w-[500px] max-w-[min(560px,100vw-32px)] p-1 flex flex-col gap-px max-h-[min(480px,100vh-140px)] overflow-auto scroll-visible"
          onOpenChange={(open) => {
            setJobsOpen(open);
            // 收起菜单即收起展开行：下次打开看到的是一份干净的列表。
            if (!open) setExpandedId(null);
          }}
          trigger={({ open, toggle }) => (
            <button
              type="button"
              onClick={() => {
                // 与打开同一提交里取时刻：挂载时的旧值会让第一帧的耗时偏小。
                setNowMs(Date.now());
                toggle();
              }}
              title="后台任务"
              className={triggerClass}
            >
              <TerminalIcon size={14} className="shrink-0" />
              <span>后台任务</span>
              <span className="mx-1">{jobs.length - Object.keys(dismissed).length}</span>
              <ChevronDownIcon
                size={12}
                className={`shrink-0 transition-transform duration-150 ease-out ${open ? 'rotate-180' : ''}`}
              />
            </button>
          )}
        >
          {active.length > 0 && <SectionHeader first label="进行中" />}
          {active.map((job) => (
            <JobLine
              key={job.id}
              job={job}
              stoppedReason={null}
              live={isLive(job)}
              expanded={expandedId === job.id}
              armed={armed === job.id}
              stopping={stopping === job.id}
              nowMs={nowMs}
              onToggle={() => setExpandedId((current) => (current === job.id ? null : job.id))}
              onStop={requestStop}
            />
          ))}
          {stopError !== null && (
            <span className="block px-2 py-1 text-meta text-danger">{stopError}</span>
          )}
          {finished.length > 0 && (
            <>
              <SectionHeader
                first={active.length === 0}
                label={`已结束 ${finished.length}`}
                fold={{
                  expanded: finishedOpen,
                  onToggle: () => setSettledOpen(!finishedOpen),
                }}
                action={{
                  label: '清空',
                  onClick: () => {
                    setDismissed((current) => {
                      const next = { ...current };
                      for (const job of finished) next[job.id] = true;
                      return next;
                    });
                    // 正在被清掉的那一行若开着面板，面板一起收起。
                    if (finished.some((job) => job.id === expandedId)) setExpandedId(null);
                  },
                }}
              />
              {finishedOpen &&
                finished.map((job) => (
                  <JobLine
                    key={job.id}
                    job={job}
                    stoppedReason={reasonOf(job)}
                    live={isLive(job)}
                    expanded={expandedId === job.id}
                    armed={false}
                    stopping={false}
                    nowMs={nowMs}
                    onToggle={() => setExpandedId((current) => (current === job.id ? null : job.id))}
                    onStop={requestStop}
                  />
                ))}
            </>
          )}
        </Dropdown>
      )}
    </>
  );
};