import React from 'react';
import { ToolContent } from './ToolContent';
import { useJobStream } from './useJobStream';

/*
 * 时间线里 `job_output` 那一行的输出区。
 *
 * 读的是作业归档本身，而不是这次工具调用的回执：回执是一串 `job output job_id=…
 * stdout=…` 的机器文本，读不出「作业跑到哪了」。窗口取最近 10 行，节拍与后台任务卡
 * 同一套（见 useJobStream）。
 *
 * 作业已被回收、归档读不到时退回回执原文：那一刻回执是唯一还带信息的载体，退出码与
 * 完整归档路径都在里面。
 */

/** 作业输出区：跟随归档增量，读不到时退回本次调用自己的回执。 */
export const JobOutputTail: React.FC<{
  laneId: string | null;
  jobId: string;
  /** 这次工具调用自己的回执原文。 */
  fallback: string;
}> = ({ laneId, jobId, fallback }) => {
  // 初值按「在跑」算：展开的一瞬间先给等待提示，不要先闪一下机器回执。
  const stream = useJobStream(laneId, jobId, true);
  // 没有车道就没有读取通道：直接退回回执，别让「正在读取」一直转下去。
  if (laneId === null) {
    return fallback.length > 0 ? <ToolContent title="输出" value={fallback} /> : null;
  }
  const note =
    stream.unavailable === null ? null : (
      <div className="px-1 pb-1 text-meta text-text-tertiary">{stream.unavailable}</div>
    );
  if (stream.text.length > 0) {
    return (
      <>
        <ToolContent title="输出" value={stream.text} />
        {note}
      </>
    );
  }
  // 失败先于「在跑」：读不到就把原因摆出来，不能让转圈把结论盖住。
  if (stream.unavailable !== null) {
    return (
      <>
        {fallback.length > 0 && <ToolContent title="输出" value={fallback} />}
        {note}
      </>
    );
  }
  if (stream.running) {
    return <div className="px-1 pb-1 text-meta text-text-tertiary">正在读取作业输出...</div>;
  }
  return (
    <>
      {fallback.length > 0 && <ToolContent title="输出" value={fallback} />}
      {note}
    </>
  );
};
