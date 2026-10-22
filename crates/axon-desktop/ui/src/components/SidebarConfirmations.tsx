import React from 'react';
import type { LaneItem, WorkspaceItem } from '../types';
import type { LaneRuntimeFacts } from '../conversation/runtimeFacts';
import { Text } from './ui/surfaces';
import { Dialog } from './ui/overlays';

/*
 * 侧栏的两个二次确认：归档正在运行的会话、移除一个工作区。
 *
 * 两个确认都把「按下之后会发生什么、不会发生什么」写在同一屏里：
 * 归档不停任务、移除不动磁盘文件。这类事实放在列表行里说不清，
 * 放在弹窗正文里一次说完，因此这里不与侧栏的其它状态共享任何文案。
 */

export interface ArchiveLaneDialogProps {
  /** 待归档的会话；为空表示没有弹窗。 */
  lane: LaneItem | null;
  /** 当前会话的运行时事实，用来摆出还在跑什么。 */
  runtime: LaneRuntimeFacts | null;
  /** 正在停止。 */
  stopping: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

/** 归档正在运行的会话：先停掉本轮与派生的子任务，再归档。 */
export const ArchiveLaneDialog: React.FC<ArchiveLaneDialogProps> = ({
  lane,
  runtime,
  stopping,
  onClose,
  onConfirm,
}) => (
  <Dialog
    open={lane !== null}
    onClose={() => {
      if (!stopping) onClose();
    }}
    title="归档正在运行的会话？"
    primary={{
      label: stopping ? '正在停止' : '停止并归档',
      busy: stopping,
      onClick: onConfirm,
    }}
    secondary={{
      label: '取消',
      disabled: stopping,
      onClick: onClose,
    }}
    hint={lane?.title ?? ''}
  >
    <Text size="meta" className="block">
      这个会话正在运行，归档不会让它自己停下来：
    </Text>
    <ul className="mt-2 space-y-1">
      {runtime?.turnRunning === true && (
        <li className="text-meta text-text-secondary">主轮仍在推进</li>
      )}
      {(runtime?.activeSubagents ?? 0) > 0 && (
        <li className="text-meta text-text-secondary">
          进行中的子代理 {runtime?.activeSubagents} 个
        </li>
      )}
      {(runtime?.activeJobs ?? 0) > 0 && (
        <li className="text-meta text-text-secondary">
          进行中的后台任务 {runtime?.activeJobs} 个
        </li>
      )}
    </ul>
    <Text size="meta" tone="tertiary" className="mt-2 block">
      确认后会先停止本轮及其派生的子任务，再把这个会话移出列表。
    </Text>
  </Dialog>
);

export interface RemoveWorkspaceDialogProps {
  /** 待移除的工作区；为空表示没有弹窗。 */
  workspace: WorkspaceItem | null;
  /** 正在移除。 */
  removing: boolean;
  /** 这个工作区里还有会话在跑（移除不会让它停下来）。 */
  busy: boolean;
  /** 用户正看着它下面的会话（移除后回到首屏）。 */
  viewed: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

/** 移除工作区：只是取消登记，磁盘上的文件一个都不动。 */
export const RemoveWorkspaceDialog: React.FC<RemoveWorkspaceDialogProps> = ({
  workspace,
  removing,
  busy,
  viewed,
  onClose,
  onConfirm,
}) => (
  <Dialog
    open={workspace !== null}
    onClose={() => {
      if (!removing) onClose();
    }}
    title="移除工作区？"
    primary={{
      label: removing ? '正在移除' : '移除',
      busy: removing,
      onClick: onConfirm,
    }}
    secondary={{
      label: '取消',
      disabled: removing,
      onClick: onClose,
    }}
    hint={workspace?.path ?? ''}
  >
    <Text size="meta" className="block">
      只把 {workspace?.name ?? ''} 移出侧栏登记，磁盘上的文件一个都不动。
    </Text>
    <ul className="mt-2 space-y-1">
      {busy && <li className="text-meta text-gold-ink">里面有会话正在运行，移除不会让它停下来</li>}
      {viewed && <li className="text-meta text-text-secondary">你正在看它下面的会话，移除后回到首屏</li>}
    </ul>
    <Text size="meta" tone="tertiary" className="mt-2 block">
      想再看到它时，用侧栏的「添加工作区」重新选中同一个目录即可。
    </Text>
  </Dialog>
);
