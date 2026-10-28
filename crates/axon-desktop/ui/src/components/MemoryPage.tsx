import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircleIcon, ArchiveIcon, BrainIcon, EditIcon, PlusIcon, TrashIcon } from '../icons';
import * as ipc from '../ipc';
import type { MemoryCursor, MemoryKind, MemoryRecord, MemoryScopeOption, MemoryStatsResult } from '../types';
import { MEMORY_KINDS } from './memoryDraft';
import { MemoryEditor } from './MemoryEditor';
import { MemoryWheel } from './MemoryWheel';
import { Button, IconButton, SegmentedControl } from './ui/controls';
import { Dialog, Select } from './ui/overlays';
import { PageShell } from './ui/shell';
import { Banner, EmptyState, LoadingState, Panel, StateDot, Text } from './ui/surfaces';

const PAGE_SIZE = 50;
const ALL_SCOPES = 'all';

interface MemoryPageProps {
  connected: boolean;
}

/** 统计条的一格：标签 12/16，数值 18/26，内部只用 0.5px 分隔。 */
const StatCell: React.FC<{ label: string; value?: number; first: boolean }> = ({
  label,
  value,
  first,
}) => (
  <div className={`px-4 py-3 ${first ? '' : 'hairline-l border-border-hairline'}`}>
    <Text tone="secondary" size="xs" className="block">
      {label}
    </Text>
    {value === undefined ? (
      <Text tone="tertiary" size="xs" className="mt-0.5 block">
        读取中
      </Text>
    ) : (
      <span className="mt-0.5 block text-heading-3 font-medium tabular-nums">{value}</span>
    )}
  </div>
);

export const MemoryPage: React.FC<MemoryPageProps> = ({ connected }) => {
  const [kind, setKind] = useState<MemoryKind>('profile');
  const [scope, setScope] = useState<string>(ALL_SCOPES);
  const [scopes, setScopes] = useState<MemoryScopeOption[]>([]);
  const [records, setRecords] = useState<MemoryRecord[]>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const [stats, setStats] = useState<MemoryStatsResult | null>(null);
  const [nextCursor, setNextCursor] = useState<MemoryCursor | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [errorDetails, setErrorDetails] = useState<string[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [needsRepair, setNeedsRepair] = useState(false);
  const [legacyTables, setLegacyTables] = useState<string[]>([]);
  const [editorFor, setEditorFor] = useState<{ record: MemoryRecord | null } | null>(null);
  const [pending, setPending] = useState<{ action: 'forget' | 'remove'; record: MemoryRecord } | null>(null);
  const [actionId, setActionId] = useState<string | null>(null);
  const kindRef = useRef<MemoryKind>(kind);
  const scopeRef = useRef<string>(scope);
  kindRef.current = kind;
  scopeRef.current = scope;

  const loadScopes = useCallback(async () => {
    try {
      const result = await ipc.memoryScopes();
      setScopes(result.scopes);
    } catch (failure) {
      setError('读取记忆作用域失败');
      setErrorDetails([failure instanceof Error ? failure.message : '未知错误']);
    }
  }, []);

  const loadStats = useCallback(async (activeScope: string) => {
    try {
      const result = await ipc.memoryStats(activeScope);
      setStats(result);
      setLegacyTables(result.legacyRecordTables);
      setNeedsRepair(result.needsRepair);
    } catch (failure) {
      setError('读取记忆统计失败');
      setErrorDetails([failure instanceof Error ? failure.message : '未知错误']);
    }
  }, []);

  const loadFirstPage = useCallback(
    async (activeScope: string, activeKind: MemoryKind) => {
      setLoading(true);
      setError(null);
      setErrorDetails([]);
      try {
        const result = await ipc.memoryList(
          activeKind,
          activeScope === ALL_SCOPES ? undefined : activeScope,
          PAGE_SIZE,
        );
        setRecords(result.records);
        setActiveIndex(0);
        setNextCursor(result.nextCursor ?? null);
        setNeedsRepair((current) => current || result.needsRepair);
      } catch (failure) {
        setError('读取记忆时间轴失败');
        setErrorDetails([failure instanceof Error ? failure.message : '未知错误']);
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  /** 时间轮推进到接近末尾时追加下一页；同一游标只请求一次。 */
  const requestMore = useCallback(async () => {
    const cursor = nextCursor;
    if (cursor === null || loadingMore) return;
    setLoadingMore(true);
    setNextCursor(null);
    try {
      const result = await ipc.memoryList(
        kindRef.current,
        scopeRef.current === ALL_SCOPES ? undefined : scopeRef.current,
        PAGE_SIZE,
        cursor,
      );
      setRecords((current) => {
        const seen = new Set(current.map((item) => item.memoryId));
        return [...current, ...result.records.filter((item) => !seen.has(item.memoryId))];
      });
      setNextCursor(result.nextCursor ?? null);
    } catch (failure) {
      setError('加载更早的记忆失败');
      setErrorDetails([failure instanceof Error ? failure.message : '未知错误']);
    } finally {
      setLoadingMore(false);
    }
  }, [loadingMore, nextCursor]);

  useEffect(() => {
    if (!connected) return;
    void loadScopes();
  }, [connected, loadScopes]);

  useEffect(() => {
    if (!connected) return;
    void loadFirstPage(scope, kind);
    void loadStats(scope);
  }, [connected, scope, kind, loadFirstPage, loadStats]);

  const scopeOptions = useMemo(
    () => [
      { value: ALL_SCOPES, label: '全部可见记忆' },
      { value: 'personal', label: '个人记忆' },
      ...scopes
        .filter((item) => item.scopeKind === 'workspace')
        .map((item) => ({ value: item.scope, label: `${item.label}（${item.recordCount}）` })),
    ],
    [scopes],
  );

  const kindOptions = useMemo(
    () =>
      MEMORY_KINDS.map((item) => ({
        value: item.value,
        label:
          stats === null
            ? item.label
            : `${item.label} ${stats.memoryKindCounts.find((entry) => entry.memoryKind === item.value)?.count ?? 0}`,
      })),
    [stats],
  );

  const applyReceipt = (result: { needsRepair: boolean }, message: string) => {
    setNotice(result.needsRepair ? `${message}；有派生索引待修复` : message);
    setNeedsRepair((current) => current || result.needsRepair);
  };

  const confirmPending = async () => {
    if (pending === null) return;
    const { action, record } = pending;
    setActionId(record.memoryId);
    setError(null);
    setErrorDetails([]);
    try {
      const receipt =
        action === 'forget'
          ? await ipc.memoryForget(record.memoryId)
          : await ipc.memoryRemove(record.memoryId);
      setRecords((current) => current.filter((item) => item.memoryId !== record.memoryId));
      setActiveIndex((current) => Math.max(0, current - 1));
      applyReceipt(receipt, action === 'forget' ? '已写入遗忘标记' : '已删除这条记忆');
      await Promise.all([loadStats(scope), loadScopes()]);
    } catch (failure) {
      setError(action === 'forget' ? '遗忘记忆失败' : '删除记忆失败');
      setErrorDetails([failure instanceof Error ? failure.message : '未知错误']);
    } finally {
      setActionId(null);
      setPending(null);
    }
  };

  if (!connected) {
    return (
      <PageShell title="记忆管理">
        <EmptyState
          icon={<BrainIcon size={24} />}
          title="未连接到 Axon 后台服务"
          description="记忆由后台服务提供；服务恢复后本页会自动重新载入，无需重新打开窗口。"
        />
      </PageShell>
    );
  }

  const pendingSummary = pending?.record.memorySummary ?? '';
  return (
    <>
      <PageShell
        title="记忆管理"
        description="时间轮按更新时间倒序排列记忆；滚轮或上下方向键切换条目。"
        actions={
          <Button
            variant="primary"
            size="sm"
            onClick={() => setEditorFor({ record: null })}
          >
            <PlusIcon size={14} />
            新增记忆
          </Button>
        }
        fill
      >
        <div className="flex flex-1 min-h-0 flex-col gap-4">
          {error !== null && (
            <Banner
              tone="danger"
              icon={<AlertCircleIcon size={14} />}
              title={error}
              details={errorDetails}
            />
          )}
          {notice !== null && (
            <Banner
              tone="success"
              icon={<BrainIcon size={14} />}
              title={notice}
              action={
                <Button size="sm" variant="ghost" onClick={() => setNotice(null)}>
                  知道了
                </Button>
              }
            />
          )}
          {needsRepair && (
            <Panel className="flex items-start gap-2 px-4 py-3">
              <StateDot state="warning" className="mt-1" />
              <div className="min-w-0 space-y-1">
                <Text size="sm" strong>
                  部分派生索引待修复
                </Text>
                <Text tone="secondary" size="xs" className="block">
                  记忆已写入，但文本索引或向量投影尚未对齐；执行 axon memory doctor 可查看明细。
                </Text>
              </div>
            </Panel>
          )}
          {legacyTables.length > 0 && (
            <Panel className="flex items-start gap-2 px-4 py-3">
              <StateDot state="warning" className="mt-1" />
              <div className="min-w-0 space-y-1">
                <Text size="sm" strong>
                  检测到升级时归档的旧记忆表：{legacyTables.join('、')}
                </Text>
                <Text tone="secondary" size="xs" className="block">
                  旧库没有可信的记忆类型字段，因此原表被整体归档保留，没有删除任何一行。
                  恢复方式：用记忆库工具打开记忆库文件，把归档表中的行按当前类型规则重新写入记忆表。
                </Text>
              </div>
            </Panel>
          )}

          <Panel className="grid grid-cols-3">
            <StatCell first label="总记录" value={stats?.totalRecords} />
            <StatCell first={false} label="活跃" value={stats?.activeRecords} />
            <StatCell first={false} label="已归档" value={stats?.archivedRecords} />
          </Panel>

          <div className="flex flex-wrap items-center justify-between gap-3">
            <SegmentedControl
              label="记忆类型"
              options={kindOptions}
              value={kind}
              onChange={setKind}
            />
            <div className="flex items-center gap-2">
              <Text tone="caption" size="xs" className="tabular-nums">
                {records.length > 0
                  ? `${activeIndex + 1} / ${records.length}${nextCursor !== null ? '+' : ''}`
                  : ''}
              </Text>
              <div className="w-64">
                <Select
                  label="记忆作用域"
                  value={scope}
                  options={scopeOptions}
                  onChange={setScope}
                />
              </div>
            </div>
          </div>

          <section aria-live="polite" className="flex flex-1 min-h-0 flex-col">
            {loading && records.length === 0 ? (
              <LoadingState label="正在读取记忆" />
            ) : records.length === 0 ? (
              <Panel>
                <EmptyState
                  icon={<BrainIcon size={24} />}
                  title={scope === ALL_SCOPES ? '这一类记忆还是空的' : '这个作用域下没有记忆'}
                  description={
                    scope === ALL_SCOPES
                      ? '新增一条，或者切换到其他类型看看。'
                      : '切换到全部可见记忆，可以看到其他作用域的真实记录。'
                  }
                  action={
                    scope === ALL_SCOPES ? (
                      <Button
                        variant="primary"
                        size="sm"
                        onClick={() => setEditorFor({ record: null })}
                      >
                        <PlusIcon size={14} />
                        新增记忆
                      </Button>
                    ) : (
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => setScope(ALL_SCOPES)}
                      >
                        查看全部记忆
                      </Button>
                    )
                  }
                />
              </Panel>
            ) : (
              <MemoryWheel
                records={records}
                activeIndex={activeIndex}
                onActiveIndexChange={setActiveIndex}
                onPrefetch={() => void requestMore()}
                loadingMore={loadingMore}
                renderActions={(record) => (
                  <>
                    <IconButton
                      label="修改记忆"
                      onClick={() => setEditorFor({ record })}
                    >
                      <EditIcon size={14} />
                    </IconButton>
                    <IconButton
                      label="遗忘记忆"
                      disabled={actionId === record.memoryId}
                      onClick={() => setPending({ action: 'forget', record })}
                    >
                      <ArchiveIcon size={14} />
                    </IconButton>
                    <IconButton
                      label="永久删除记忆"
                      disabled={actionId === record.memoryId}
                      onClick={() => setPending({ action: 'remove', record })}
                    >
                      <TrashIcon size={14} />
                    </IconButton>
                  </>
                )}
              />
            )}
          </section>
        </div>
      </PageShell>

      {editorFor !== null && (
        <MemoryEditor
          record={editorFor.record}
          defaultKind={kind}
          onClose={() => setEditorFor(null)}
          onSaved={(repair, count) => {
            setEditorFor(null);
            setNotice(
              repair
                ? `已写入 ${count} 条记忆；有派生索引待修复`
                : `已写入 ${count} 条记忆`,
            );
            setNeedsRepair((current) => current || repair);
            void loadFirstPage(scope, kind);
            void loadStats(scope);
            void loadScopes();
          }}
        />
      )}

      <Dialog
        open={pending !== null}
        onClose={() => {
          if (actionId === null) setPending(null);
        }}
        title={pending?.action === 'remove' ? '永久删除这条记忆？' : '遗忘这条记忆？'}
        hint={pendingSummary.length > 0 ? pendingSummary : undefined}
        primary={{
          label: pending?.action === 'remove' ? '确认删除' : '确认遗忘',
          busyLabel: pending?.action === 'remove' ? '删除中' : '处理中',
          busy: actionId !== null,
          icon:
            pending?.action === 'remove' ? (
              <TrashIcon size={14} />
            ) : (
              <ArchiveIcon size={14} />
            ),
          onClick: () => void confirmPending(),
        }}
        secondary={{
          label: '取消',
          disabled: actionId !== null,
          onClick: () => setPending(null),
        }}
      >
        <Text size="sm" tone="secondary" className="block leading-relaxed">
          {pending?.action === 'remove'
            ? '删除后无法恢复，也不会留下遗忘标记。'
            : '遗忘后这条记忆不再参与检索，系统会保留一条遗忘标记，阻止它被重新写入。'}
        </Text>
      </Dialog>
    </>
  );
};
