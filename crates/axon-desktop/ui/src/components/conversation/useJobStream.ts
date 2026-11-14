import React from 'react';
import * as ipc from '../../ipc';

/*
 * 后台作业输出的跟随读取。
 *
 * 作业的真实输出在运行侧的归档文件里，网关按字节游标增量给出；这里只负责按节拍
 * 把增量拼起来，渲染与滚动跟随由终端卡自己决定。转录里那条工具回执（`job start
 * job_id=…`）是调用事实，不是作业输出，因此不参与这里的拼接。
 */

/** 跟随节奏：作业在跑时每秒一帧。够快能看出「在动」，又不至于把网关刷爆。 */
export const JOB_TAIL_INTERVAL_MS = 1_000;

/**
 * 跟随窗口：只保留最近这么多行。
 *
 * 终端卡回答的是「它跑到哪了」，不是「读完整份日志」：一个每秒刷新的窗口只要够看懂
 * 当下的进展与收尾结论即可。全量输出在作业归档里，需要时去那里取。
 */
export const JOB_OUTPUT_TAIL_LINES = 10;

/** 连续失败到这个次数就停下：网关长时间不可达时，空转的请求本身也是负担。 */
const JOB_TAIL_MAX_FAILURES = 5;

export interface JobStream {
  /** 已累积的输出正文。 */
  text: string;
  /** 读不到的原因；为空表示一切正常。 */
  unavailable: string | null;
  /** 作业是否仍在运行（以网关为准，比转录里的启动快照新）。 */
  running: boolean;
}

/** 尚未观测到任何一帧时的状态。 */
function emptyStream(running: boolean): JobStream {
  return { text: '', unavailable: null, running };
}

/**
 * 只留最近 `maxLines` 行。
 *
 * 从尾部反向数换行符而不是整段 split：每帧都要跑一次，省掉一次几万字符的数组分配。
 * 换行不足时原样返回；末尾没有换行的那一行同样算一行，正在写的半行不会被切掉。
 */
function tailLines(text: string, maxLines: number): string {
  let cursor = text.length;
  let seen = 0;
  while (cursor > 0) {
    cursor = text.lastIndexOf('\n', cursor - 1);
    if (cursor < 0) return text;
    seen += 1;
    if (seen === maxLines) return text.slice(cursor + 1);
  }
  return text;
}

/**
 * 跟随一个后台作业的输出。
 *
 * `fallbackRunning` 只在第一帧到达之前用作初值：转录里的状态是启动那一刻的快照，
 * 真实运行态由网关回答。作业收口（不再运行且已到归档末尾）或归档不可读时停止轮询，
 * 已累积的内容原样保留。
 */
export function useJobStream(
  laneId: string | null,
  jobId: string | null,
  fallbackRunning = false,
): JobStream {
  const initialRunning = React.useRef(fallbackRunning);
  const [stream, setStream] = React.useState<JobStream>(() => emptyStream(fallbackRunning));
  const offsetRef = React.useRef(0);
  const attachedRef = React.useRef<string | null>(null);

  React.useEffect(() => {
    initialRunning.current = fallbackRunning;
  }, [fallbackRunning]);

  React.useEffect(() => {
    if (laneId === null || jobId === null) {
      offsetRef.current = 0;
      attachedRef.current = null;
      setStream(emptyStream(initialRunning.current));
      return undefined;
    }
    // 换作业就整段重来：游标属于某一个具体作业，跨作业复用会把两段输出拼在一起。
    const key = `${laneId}\u0000${jobId}`;
    if (attachedRef.current !== key) {
      attachedRef.current = key;
      offsetRef.current = 0;
      setStream(emptyStream(initialRunning.current));
    }
    let cancelled = false;
    let timer: number | null = null;
    let failures = 0;
    const pull = () => {
      // 页面不可见时不发请求：切走或最小化期间没人看这一格，省下的是纯流量。
      if (document.visibilityState === 'hidden') {
        timer = window.setTimeout(pull, JOB_TAIL_INTERVAL_MS);
        return;
      }
      void ipc
        .conversationJobOutput(laneId, jobId, offsetRef.current)
        .then((page) => {
          if (cancelled) return;
          failures = 0;
          offsetRef.current = page.nextOffset;
          setStream((current) => {
            const unavailable = page.unavailable ?? current.unavailable;
            // 没有新内容也没有状态变化时返回同一个引用：React 据此跳过这次重渲染。
            if (
              page.text.length === 0 &&
              page.running === current.running &&
              unavailable === current.unavailable
            ) {
              return current;
            }
            return {
              // 每帧都重新收成最近若干行：窗口大小恒定，读数不会越滚越长。
              text: tailLines(current.text + page.text, JOB_OUTPUT_TAIL_LINES),
              unavailable,
              running: page.running,
            };
          });
          // 只有「不再运行且已到归档末尾」才算收口：作业在跑时 eof 同样会为真。
          const settled = page.unavailable !== null || (!page.running && page.eof);
          if (!settled) timer = window.setTimeout(pull, JOB_TAIL_INTERVAL_MS);
        })
        .catch((failure: unknown) => {
          if (cancelled) return;
          failures += 1;
          // 一次抖动不改变结论：下一拍再试；连续失败才收手。
          if (failures < JOB_TAIL_MAX_FAILURES) {
            timer = window.setTimeout(pull, JOB_TAIL_INTERVAL_MS);
            return;
          }
          // 收手必须可见：静默停在旧内容上，读起来像「作业没有输出了」。
          const reason = failure instanceof Error ? failure.message : String(failure);
          setStream((current) => ({
            ...current,
            unavailable: reason.trim().length > 0 ? reason.trim() : '读不到作业输出',
          }));
        });
    };
    pull();
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [laneId, jobId]);

  return stream;
}
