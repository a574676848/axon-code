import React from "react";
import * as ipc from "../ipc";
import type { CronConfigResult, CronHistoryResult, CronJob } from "../types";
import {
  AlertCircleIcon,
  CloseIcon,
  PlusIcon,
  RefreshIcon,
  SettingsIcon,
  TrashIcon,
} from "../icons";
import { Button, IconButton } from "./ui/controls";
import { Dialog } from "./ui/overlays";
import { PageShell } from "./ui/shell";
import { Banner, FormField, LoadingState, Text } from "./ui/surfaces";
import { AutomationDetail } from "./automation/AutomationDetail";
import {
  EMPTY_FORM,
  formFromJob,
  requestFromForm,
  wakeLabel,
  type FormState,
} from "./automation/jobForm";
import { AutomationList, type JobFilter } from "./automation/AutomationList";
import {
  emptySchedule,
  encodeSchedule,
  parseSchedule,
  scheduleError,
  type ScheduleForm,
} from "./automation/schedule";

const PAGE_SIZE = 20;
const RUN_POLL_MS = 2_000;

/**
 * 内容区高度：视口减去 PageShell 的顶栏 36px、上下内边距 40px、
 * 一行说明 16px 及其下外边距 16px，向上取整留 4px 余量（7rem）。
 * 高度必须在这里给定：只有容器高度确定，左列与右列才能各自成为独立滚动区，
 * 页面本身也就不会出现第二层滚动。
 */
/** 内容区直接吃满舞台余下高度：页壳已经给了确定高度，不再自己算 100vh。 */
const CONTENT_HEIGHT = "h-full";

const pageCount = (total: number, size: number) =>
  Math.max(1, Math.ceil(total / size));

export const AutomationPage: React.FC<{
  onOpenSession: (job: CronJob) => void;
}> = ({ onOpenSession }) => {
  const [jobs, setJobs] = React.useState<CronJob[]>([]);
  const [jobPage, setJobPage] = React.useState(1);
  const [jobTotal, setJobTotal] = React.useState(0);
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [history, setHistory] = React.useState<CronHistoryResult | null>(null);
  const [historyPage, setHistoryPage] = React.useState(1);
  const [historyLoading, setHistoryLoading] = React.useState(false);
  const [historyError, setHistoryError] = React.useState<string | null>(null);
  const [query, setQuery] = React.useState("");
  const [filter, setFilter] = React.useState<JobFilter>("all");
  const [form, setForm] = React.useState<FormState>(EMPTY_FORM);
  const [scheduleForm, setScheduleForm] =
    React.useState<ScheduleForm>(emptySchedule());
  const [editing, setEditing] = React.useState(false);
  const [config, setConfig] = React.useState<CronConfigResult | null>(null);
  const [configLoading, setConfigLoading] = React.useState(false);
  const [configError, setConfigError] = React.useState<string | null>(null);
  const [showConfig, setShowConfig] = React.useState(false);
  const [showHistory, setShowHistory] = React.useState(false);
  const [confirmDelete, setConfirmDelete] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [loaded, setLoaded] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const serial = React.useRef(0);
  const historySerial = React.useRef(0);
  const selected = jobs.find((job) => job.jobId === selectedId) ?? null;
  const refresh = React.useCallback(
    async (page = jobPage) => {
      const request = ++serial.current;
      try {
        const result = await ipc.cronList(
          page,
          PAGE_SIZE,
          filter === "all" ? "" : filter,
          query.trim(),
        );
        if (request !== serial.current) return;
        setJobs(result.jobs);
        setJobPage(
          result.total > 0
            ? Math.min(result.page, pageCount(result.total, PAGE_SIZE))
            : 1,
        );
        setJobTotal(result.total);
        setSelectedId((current) =>
          current && result.jobs.some((job) => job.jobId === current)
            ? current
            : null,
        );
        setError(null);
      } catch (failure) {
        if (request === serial.current)
          setError(
            failure instanceof Error ? failure.message : "读取自动化任务失败",
          );
      } finally {
        if (request === serial.current) setLoaded(true);
      }
    },
    [filter, jobPage, query],
  );
  React.useEffect(() => {
    setJobPage(1);
  }, [filter, query]);
  React.useEffect(() => {
    void refresh(jobPage);
  }, [refresh, jobPage]);

  /** 轮询只静默刷新历史，不把已经展开的列表打回读取中。 */
  const refreshHistorySilently = React.useCallback(
    (jobId: string, page: number) => {
      const request = ++historySerial.current;
      void ipc
        .cronHistory(jobId, page, PAGE_SIZE)
        .then((result) => {
          if (request !== historySerial.current) return;
          setHistory(result);
          setHistoryError(null);
        })
        .catch((failure) => {
          if (request === historySerial.current)
            setHistoryError(
              failure instanceof Error ? failure.message : "读取执行历史失败",
            );
        });
    },
    [],
  );

  const hasRunningJob = jobs.some((job) => job.running === true);
  React.useEffect(() => {
    if (!hasRunningJob) return;
    const timer = window.setInterval(() => {
      void refresh(jobPage);
      if (selectedId !== null && showHistory) {
        refreshHistorySilently(selectedId, historyPage);
      }
    }, RUN_POLL_MS);
    return () => window.clearInterval(timer);
  }, [
    hasRunningJob,
    refresh,
    jobPage,
    selectedId,
    showHistory,
    historyPage,
    refreshHistorySilently,
  ]);

  React.useEffect(() => {
    ++historySerial.current;
    setHistoryLoading(false);
    setHistoryError(null);
    if (!selected) return;
    setForm(formFromJob(selected));
    setScheduleForm(parseSchedule(selected.schedule));
    setEditing(false);
    setShowHistory(false);
    setHistory(null);
  }, [selectedId]);

  const mutate = async (
    action: () => Promise<unknown>,
    success?: () => void,
  ) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      success?.();
      await refresh(jobPage);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : "操作失败");
    } finally {
      setBusy(false);
    }
  };

  const create = () => {
    setSelectedId(null);
    setForm(EMPTY_FORM);
    setScheduleForm(emptySchedule());
    setEditing(true);
    setHistory(null);
    setShowHistory(false);
  };

  const save = () =>
    void mutate(
      () => {
        const validation = scheduleError(scheduleForm);
        if (validation) return Promise.reject(new Error(validation));
        const next = { ...form, schedule: encodeSchedule(scheduleForm) };
        return selected
          ? ipc.cronUpdate(selected.jobId, {
              ...requestFromForm(next),
              replaceWake: true,
              replaceWorkdir: true,
              replaceScript: true,
            })
          : ipc.cronCreate(requestFromForm(next));
      },
      () => setEditing(false),
    );

  const loadHistory = (page = 1) => {
    if (!selected) return;
    setShowHistory(true);
    setHistoryPage(page);
    setHistoryLoading(true);
    setHistoryError(null);
    const request = ++historySerial.current;
    void ipc
      .cronHistory(selected.jobId, page, PAGE_SIZE)
      .then((result) => {
        if (request !== historySerial.current) return;
        setHistory(result);
      })
      .catch((failure) => {
        if (request !== historySerial.current) return;
        setHistoryError(
          failure instanceof Error ? failure.message : "读取执行历史失败",
        );
      })
      .finally(() => {
        if (request === historySerial.current) setHistoryLoading(false);
      });
  };

  const loadConfig = () => {
    setShowConfig(true);
    setConfigLoading(true);
    setConfigError(null);
    void ipc
      .cronConfig()
      .then(setConfig)
      .catch((failure) =>
        setConfigError(
          failure instanceof Error ? failure.message : "读取配置失败",
        ),
      )
      .finally(() => setConfigLoading(false));
  };

  const selectedJobId = selected?.jobId;
  const splitView = selected !== null || editing;
  const showSystemChip =
    jobs.some((job) => job.system === true) &&
    jobs.some((job) => job.system !== true);
  return (
    <>
      <PageShell
        title="自动化任务"
        description={
          selected === null
            ? "按计划运行的任务。选择一条任务查看计划与执行历史。"
            : "查看任务的调度、任务内容与执行历史。"
        }
        actions={
          <>
            <IconButton
              label="刷新任务"
              onClick={() => void refresh()}
              disabled={busy}
            >
              <RefreshIcon size={14} />
            </IconButton>
            <Button variant="outline" size="sm" onClick={loadConfig}>
              <SettingsIcon size={14} />
              配置详情
            </Button>
            <Button variant="primary" size="sm" disabled={busy} onClick={create}>
              <PlusIcon size={14} />
              新建任务
            </Button>
          </>
        }
        fill
      >
        <div className={`flex min-h-0 flex-col gap-3 ${CONTENT_HEIGHT}`}>
          {error !== null && (
            <div className="shrink-0">
              <Banner
                tone="danger"
                icon={<AlertCircleIcon size={14} />}
                title={error}
              />
            </div>
          )}
          <div className="flex-1 min-h-0 flex items-stretch gap-4">
            <AutomationList
              jobs={jobs}
              selectedId={selectedId}
              query={query}
              filter={filter}
              page={jobPage}
              total={jobTotal}
              pageSize={PAGE_SIZE}
              busy={busy}
              loading={!loaded}
              showSystemChip={showSystemChip}
              className={
                splitView ? "w-[360px] shrink-0 max-xl:hidden" : "flex-1 min-h-0"
              }
              onQueryChange={setQuery}
              onFilterChange={setFilter}
              onSelect={(jobId) => {
                setSelectedId(jobId);
                setShowConfig(false);
              }}
              onCreate={create}
              onClearFilters={() => {
                setQuery("");
                setFilter("all");
              }}
              onPageChange={(page) => void refresh(page)}
            />
            {splitView && (
              <AutomationDetail
                selected={selected}
                editing={editing}
                form={form}
                scheduleForm={scheduleForm}
                history={history}
                historyLoading={historyLoading}
                historyError={historyError}
                historyPage={historyPage}
                pageSize={PAGE_SIZE}
                showHistory={showHistory}
                busy={busy}
                className="flex-1 min-w-0 xl:min-w-[400px]"
                onFormChange={setForm}
                onScheduleChange={setScheduleForm}
                onEdit={() => {
                  if (selected) {
                    setForm(formFromJob(selected));
                    setScheduleForm(parseSchedule(selected.schedule));
                  }
                  setEditing(true);
                }}
                onCancel={() => {
                  setEditing(false);
                  if (selected === null) {
                    setSelectedId(null);
                    setForm(EMPTY_FORM);
                    setScheduleForm(emptySchedule());
                  } else {
                    setForm(formFromJob(selected));
                    setScheduleForm(parseSchedule(selected.schedule));
                  }
                }}
                onClose={() => {
                  setSelectedId(null);
                  setEditing(false);
                  setShowHistory(false);
                  setHistory(null);
                  setHistoryError(null);
                  setHistoryLoading(false);
                  ++historySerial.current;
                }}
                onSave={save}
                onPause={() =>
                  selectedJobId && void mutate(() => ipc.cronPause(selectedJobId))
                }
                onResume={() =>
                  selectedJobId &&
                  void mutate(() => ipc.cronResume(selectedJobId))
                }
                onRun={() =>
                  selectedJobId && void mutate(() => ipc.cronRun(selectedJobId))
                }
                onDelete={() => setConfirmDelete(true)}
                onHistory={(page) => loadHistory(page)}
                onOpenSession={onOpenSession}
              />
            )}
          </div>
        </div>
      </PageShell>
      <Dialog
        open={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title="删除自动化任务？"
        primary={{
          label: busy ? "删除中" : "确认删除",
          icon: <TrashIcon size={14} />,
          busy,
          onClick: () =>
            selected &&
            void mutate(
              () => ipc.cronRemove(selected.jobId),
              () => {
                setConfirmDelete(false);
                setSelectedId(null);
              },
            ),
        }}
        secondary={{
          label: "取消",
          icon: <CloseIcon size={14} />,
          disabled: busy,
          onClick: () => setConfirmDelete(false),
        }}
        hint={selected?.name ?? ""}
      >
        <Text size="sm" tone="secondary">
          删除后任务不会再被调度，执行历史会保留。
        </Text>
      </Dialog>
      <Dialog
        open={showConfig}
        onClose={() => setShowConfig(false)}
        title="Axon Cron 配置详情"
        primary={{
          label: "关闭",
          icon: <CloseIcon size={14} />,
          onClick: () => setShowConfig(false),
        }}
      >
        {configError !== null ? (
          <Banner
            tone="danger"
            icon={<AlertCircleIcon size={14} />}
            title={configError}
            action={
              <Button variant="outline" size="sm" onClick={loadConfig}>
                重试
              </Button>
            }
          />
        ) : config !== null ? (
          <div className="space-y-3">
            <FormField label="Axon Root">
              <Text size="sm" mono className="block break-all">
                {config.axonRoot}
              </Text>
            </FormField>
            <FormField label="配置文件">
              <Text size="sm" mono className="block break-all">
                {config.configPath}
              </Text>
            </FormField>
            <FormField label="事实源">
              <Text size="sm" mono className="block break-all">
                {config.storePath}
              </Text>
            </FormField>
            <FormField label="系统任务">
              <Text size="sm" className="block break-all">
                {config.systemJobIds.join("、")}
              </Text>
            </FormField>
            <FormField label="默认通知 chat_id">
              <Text
                size="sm"
                mono={config.defaultNotificationChatId != null}
                className="block break-all"
                tone={config.defaultNotificationChatId != null ? "primary" : "tertiary"}
              >
                {wakeLabel(config.defaultNotificationChatId)}
              </Text>
              <Text size="xs" tone="tertiary" className="block break-words">
                仅作提示：任务的实际投递目标永远来自任务自己声明的 wake。
              </Text>
            </FormField>
          </div>
        ) : configLoading ? (
          <LoadingState label="正在读取配置" />
        ) : null}
      </Dialog>
    </>
  );
};
