import React from "react";
import type { CronHistoryResult, CronJob } from "../../types";
import {
  ArrowUpIcon,
  ClockIcon,
  CloseIcon,
  EditIcon,
  MoreIcon,
  OpenInAppIcon,
  StopIcon,
  TrashIcon,
  TriangleRightFillIcon,
} from "../../icons";
import { Button, IconButton } from "../ui/controls";
import { Dropdown, MenuItem, MenuSeparator } from "../ui/overlays";
import {
  FormField,
  Panel,
  SectionHeading,
  StateDot,
  Text,
} from "../ui/surfaces";
import { formatMoment } from "../pageFormat";
import { AutomationEditor, type FormState } from "./AutomationEditor";
import { AutomationHistory } from "./AutomationHistory";
import { scheduleText, type ScheduleForm } from "./schedule";
import { jobState, statusText } from "./status";

export type { FormState } from "./AutomationEditor";

export interface AutomationDetailProps {
  selected: CronJob | null;
  editing: boolean;
  form: FormState;
  scheduleForm: ScheduleForm;
  history: CronHistoryResult | null;
  historyLoading: boolean;
  historyError: string | null;
  historyPage: number;
  pageSize: number;
  showHistory: boolean;
  busy: boolean;
  className?: string;
  onFormChange: (form: FormState) => void;
  onScheduleChange: (value: ScheduleForm) => void;
  onEdit: () => void;
  onCancel: () => void;
  onClose: () => void;
  onSave: () => void;
  onPause: () => void;
  onResume: () => void;
  onRun: () => void;
  onDelete: () => void;
  onHistory: (page?: number) => void;
  onOpenSession: (job: CronJob) => void;
}

/**
 * 任务详情：调度、任务内容、执行历史三段纵向排列，动作固定在卡片底部。
 * 卡片自身是唯一的滚动容器，页面不再出现第二层滚动。
 */
export const AutomationDetail: React.FC<AutomationDetailProps> = ({
  selected,
  editing,
  form,
  scheduleForm,
  history,
  historyLoading,
  historyError,
  historyPage,
  pageSize,
  showHistory,
  busy,
  className = "",
  onFormChange,
  onScheduleChange,
  onEdit,
  onCancel,
  onClose,
  onSave,
  onPause,
  onResume,
  onRun,
  onDelete,
  onHistory,
  onOpenSession,
}) => {
  const title = selected?.name ?? "新建自动化任务";
  const updateForm = (patch: Partial<FormState>) =>
    onFormChange({ ...form, ...patch });
  return (
    <Panel clip className={`flex min-h-0 flex-col ${className}`}>
      <header className="shrink-0 flex items-start gap-2 px-4 pt-4">
        <div className="min-w-0 flex-1">
          <h2
            className="text-sm font-medium leading-5 text-text-primary line-clamp-2"
            title={title}
          >
            {title}
          </h2>
          <div className="mt-1 flex items-center gap-1.5 min-w-0">
            {selected !== null ? (
              <>
                <StateDot state={jobState(selected)} />
                <Text tone="secondary" size="xs">
                  {statusText(selected.status)}
                </Text>
                <Text
                  tone="caption"
                  size="xs"
                  mono
                  truncate
                  className="ml-auto min-w-0"
                  title={selected.jobId}
                >
                  {selected.jobId}
                </Text>
              </>
            ) : (
              <Text tone="caption" size="xs">
                尚未保存
              </Text>
            )}
          </div>
        </div>
        <IconButton label="关闭任务详情" onClick={onClose} disabled={busy}>
          <CloseIcon size={14} />
        </IconButton>
      </header>
      <div className="flex-1 min-h-0 overflow-y-auto scroll-visible px-4 pt-4 pb-4">
        {editing ? (
          <AutomationEditor
            form={form}
            scheduleForm={scheduleForm}
            busy={busy}
            onFormChange={updateForm}
            onScheduleChange={onScheduleChange}
            onCancel={onCancel}
            onSave={onSave}
          />
        ) : selected !== null ? (
          <DetailBody
            job={selected}
            history={history}
            historyLoading={historyLoading}
            historyError={historyError}
            historyPage={historyPage}
            pageSize={pageSize}
            showHistory={showHistory}
            busy={busy}
            onHistory={onHistory}
          />
        ) : null}
      </div>
      {!editing && selected !== null && (
        <footer className="shrink-0 flex items-center gap-2 px-4 py-3 hairline-t border-border-hairline">
          <Button
            variant="outline"
            size="sm"
            disabled={busy || selected.running === true}
            onClick={onRun}
          >
            <ArrowUpIcon size={14} />
            {selected.running === true ? "执行中" : "立即执行"}
          </Button>
          {selected.system === true ? null : (
            <Dropdown
              align="end"
              menuClassName="min-w-[190px]"
              trigger={({ toggle, open }) => (
                <Button
                  variant="outline"
                  size="sm"
                  className={open ? "bg-hover text-text-primary" : ""}
                  onClick={toggle}
                >
                  <MoreIcon size={14} />
                  更多
                </Button>
              )}
            >
              {selected.status === "active" && (
                <MenuItem icon={<StopIcon size={14} />} onSelect={onPause}>
                  暂停
                </MenuItem>
              )}
              {selected.status === "paused" && (
                <MenuItem
                  icon={<TriangleRightFillIcon size={14} />}
                  onSelect={onResume}
                >
                  恢复
                </MenuItem>
              )}
              <MenuItem icon={<EditIcon size={14} />} onSelect={onEdit}>
                编辑
              </MenuItem>
              {!selected.noAgent && selected.workspaceId != null && (
                <MenuItem
                  icon={<OpenInAppIcon size={14} />}
                  onSelect={() => onOpenSession(selected)}
                >
                  进入会话
                </MenuItem>
              )}
              <MenuSeparator />
              <MenuItem danger icon={<TrashIcon size={14} />} onSelect={onDelete}>
                删除
              </MenuItem>
            </Dropdown>
          )}
        </footer>
      )}
    </Panel>
  );
};

const DetailBody: React.FC<{
  job: CronJob;
  history: CronHistoryResult | null;
  historyLoading: boolean;
  historyError: string | null;
  historyPage: number;
  pageSize: number;
  showHistory: boolean;
  busy: boolean;
  onHistory: (page?: number) => void;
}> = ({ job, history, historyLoading, historyError, historyPage, pageSize, showHistory, busy, onHistory }) => (
  <div className="space-y-5">
    <section className="space-y-2">
      <SectionHeading
        title="调度"
        description="按 Gateway 所在设备的本地时区执行。"
      />
      <div className="grid grid-cols-2 gap-3">
        <FormField label="执行计划">
          <Text size="sm">{scheduleText(job.schedule)}</Text>
        </FormField>
        <FormField label="状态">
          <span className="flex items-center gap-1.5">
            <StateDot state={jobState(job)} />
            <Text size="sm">{statusText(job.status)}</Text>
          </span>
        </FormField>
        <FormField label="下次执行">
          <Text size="sm" className="tabular-nums">
            {formatMoment(job.nextRunAtMs)}
          </Text>
        </FormField>
        <FormField label="上次执行">
          <Text size="sm" className="tabular-nums">
            {formatMoment(job.lastRunAtMs)}
          </Text>
        </FormField>
      </div>
    </section>
    <section className="space-y-2">
      <SectionHeading
        title="任务内容"
        description={
          job.noAgent === true
            ? "脚本任务，不调用 Agent。"
            : "每次调度交给 Agent 的提示词。"
        }
      />
      <FormField label={job.noAgent === true ? "脚本" : "提示词"}>
        <pre className="text-sm leading-5 text-text-primary whitespace-pre-wrap break-words font-sans">
          {job.noAgent === true ? job.script : job.prompt}
        </pre>
      </FormField>
      {job.skills.length > 0 && (
        <FormField label="Skills">
          <Text size="sm">{job.skills.join("、")}</Text>
        </FormField>
      )}
      {job.workdir != null && job.workdir.length > 0 && (
        <FormField label="工作目录">
          <Text size="sm" mono className="block break-all">
            {job.workdir}
          </Text>
        </FormField>
      )}
      {job.delivery.recipientId.length > 0 && (
        <FormField label="投递目标">
          <Text size="sm" mono className="block break-all">
            {job.delivery.recipientId}
          </Text>
        </FormField>
      )}
      {job.lastRunSuccess === false && (
        <FormField label="上次失败原因">
          <Text size="sm" tone="danger" className="block break-words">
            {job.lastRunError ?? "原因未知"}
          </Text>
        </FormField>
      )}
    </section>
    <section className="space-y-2">
      <SectionHeading
        title="执行历史"
        description="每次调度都会留下一条记录。"
        action={
          <Button
            variant="ghost"
            size="sm"
            disabled={busy}
            onClick={() => onHistory(showHistory ? historyPage : 1)}
          >
            <ClockIcon size={14} />
            {showHistory ? "刷新" : "查看历史"}
          </Button>
        }
      />
      {showHistory && (
        <AutomationHistory
          history={history}
          loading={historyLoading}
          error={historyError}
          page={historyPage}
          pageSize={pageSize}
          onLoad={(page) => onHistory(page)}
        />
      )}
    </section>
  </div>
);
