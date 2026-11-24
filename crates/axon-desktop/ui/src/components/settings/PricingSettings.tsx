import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Button, IconButton, Input, Switch } from '../ui/controls';
import { Dialog } from '../ui/overlays';
import { Pagination } from '../ui/pagination';
import { Banner, EmptyState, LoadingState, Panel, Text, ToggleField } from '../ui/surfaces';
import {
  AlertCircleIcon,
  CheckIcon,
  EditIcon,
  PlusIcon,
  RefreshIcon,
  SearchIcon,
  TrashIcon,
} from '../../icons';
import * as ipc from '../../ipc';
import type { PriceRule, PriceRuleInput, PricingAutoSyncReport, SyncPricingResult } from '../../types';
import { PricingRuleDialog } from './PricingRuleDialog';
import { PanelHeader } from './PanelHeader';
import { SettingsHeaderAction } from './SettingsHeaderSlot';
import { scheduleText } from '../automation/schedule';
import {
  filterRules,
  formatAgo,
  formatMultiplier,
  formatPriceCellFixed,
  formatRuleTime,
  pricingStyleLabel,
} from './pricingFormat';

/*
 * 模型定价：价格规则的读、写与每日自动同步。
 *
 * 这一页回答三件事：现在有哪些规则、怎么加一条、以及要不要让 Axon 每天自己去
 * models.dev 拉一次费率。
 *
 * Axon 不预设模型价格是产品口径而不是缺陷：没有规则时成本栏显示「未定价」，
 * 这一页就是把那些「未定价」变成金额的地方。因此空态不是一句「暂无数据」，
 * 而是把可执行的下一步摆出来。
 *
 * 规则总量在几百条量级，一次读回本地后按页显示：搜索与翻页都不再打网关。
 * 表格收在七列内：缓存读取与写入合成一列两行，优先级与更新时间收进匹配式的第二行，
 * 897px 的可用宽度里不出现横向滚动。
 */

/** 每页条数档位。 */
const PAGE_SIZES = [20, 50, 100] as const;

/** 自动同步状态行的文案。 */
function autoSyncStatusText(report: PricingAutoSyncReport | null, now: number): string {
  if (report === null) return '读取中';
  if (!report.jobExists) return '未开启，开启后由 Axon Cron 每天同步一次';
  const base = report.enabled ? '已开启 · 每天同步一次' : '已暂停 · 任务与历史记录保留';
  const last = `上次运行 ${formatAgo(report.lastRunAtMs, now)}`;
  const outcome =
    report.lastRunAtMs === null || report.lastRunAtMs === undefined
      ? ''
      : report.lastRunSuccess === true
        ? '（成功）'
        : report.lastRunSuccess === false
          ? '（失败）'
          : '';
  return `${base} · ${last}${outcome}`;
}

/**
 * 同步回执的文案。
 *
 * 「写入 0 条」有四种原因，各自的下一步动作完全不同：等一会儿、明天再说、
 * 去查网络、或者本来就已经是最新的。这里按后端给出的结论分别成句，
 * 不合并成一句「同步失败或没有更新」。
 */
function syncNotice(result: SyncPricingResult): string {
  const source = result.source;
  switch (result.outcome) {
    case 'success':
      return result.syncedRulesCount > 0
        ? `已从 ${source} 写入 ${result.syncedRulesCount} 条规则`
        : `已从 ${source} 读到费率，但没有可写入的条目`;
    case 'debounced':
      return `距上次手动同步不到一分钟，本次未执行（${result.detail ?? '防抖中'}）`;
    case 'rate_limited':
      return `自动同步每天最多一次，本次未执行（${result.detail ?? '限流中'}）`;
    default:
      return `上游不可达，本地规则保持原样：${result.detail ?? '未给出原因'}`;
  }
}

export const PricingSettings: React.FC = () => {
  const [rules, setRules] = useState<PriceRule[] | null>(null);
  const [autoSync, setAutoSync] = useState<PricingAutoSyncReport | null>(null);
  const [query, setQuery] = useState('');
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState<number>(PAGE_SIZES[0]);
  const [busy, setBusy] = useState<'sync' | 'recalculate' | 'auto' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editing, setEditing] = useState<PriceRule | null>(null);
  const [creating, setCreating] = useState(false);
  const [removing, setRemoving] = useState<PriceRule | null>(null);

  const load = useCallback(async () => {
    try {
      const [list, sync] = await Promise.all([ipc.pricingRules(), ipc.pricingAutoSync()]);
      setRules(list);
      setAutoSync(sync);
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '读取价格规则失败');
      setRules([]);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(() => filterRules(rules ?? [], query), [rules, query]);
  const pageCount = Math.max(1, Math.ceil(filtered.length / pageSize));
  const current = Math.min(page, pageCount);
  const visible = filtered.slice((current - 1) * pageSize, current * pageSize);

  /** 规则落库后统一重读：界面永远显示服务端的真值。 */
  const applyRule = async (input: PriceRuleInput, replacing: PriceRule | null): Promise<boolean> => {
    setError(null);
    setNotice(null);
    try {
      await ipc.pricingSaveRule(input);
      if (replacing !== null) {
        // 先建后删：新建成功才删除原规则，中途失败时原规则原样保留。
        await ipc.pricingDeleteRule(replacing.id);
      }
      await load();
      setNotice(
        replacing === null
          ? `已新增规则 ${input.modelPattern}`
          : `已用新规则替换 ${replacing.modelPattern}`,
      );
      return true;
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '保存价格规则失败');
      return false;
    }
  };

  const removeRule = async (rule: PriceRule) => {
    setError(null);
    setNotice(null);
    try {
      await ipc.pricingDeleteRule(rule.id);
      await load();
      setNotice(`已删除规则 ${rule.modelPattern}`);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '删除价格规则失败');
    } finally {
      setRemoving(null);
    }
  };

  const runSync = async () => {
    setBusy('sync');
    setError(null);
    setNotice(null);
    try {
      const result = await ipc.pricingSync();
      await load();
      setNotice(syncNotice(result));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '同步失败');
    } finally {
      setBusy(null);
    }
  };

  const runRecalculate = async () => {
    setBusy('recalculate');
    setError(null);
    setNotice(null);
    try {
      const result = await ipc.pricingRecalculate();
      setNotice(
        `已按当前规则重算 ${result.recalculatedCount} 条请求事实，耗时 ${result.elapsedMs} ms；用量看板的成本读数随下一次刷新更新`,
      );
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '历史重算失败');
    } finally {
      setBusy(null);
    }
  };

  const toggleAutoSync = async (enabled: boolean) => {
    setBusy('auto');
    setError(null);
    setNotice(null);
    try {
      const report = await ipc.pricingSetAutoSync(enabled);
      setAutoSync(report);
      setNotice(
        enabled
          ? '已开启：Axon Cron 每天同步一次 models.dev 费率'
          : '已关闭：同步任务转为暂停，历史运行记录保留',
      );
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '切换自动同步失败');
      // 失败就回到服务端的真实状态，不把开关停在未生效的位置上。
      try {
        setAutoSync(await ipc.pricingAutoSync());
      } catch {
        setAutoSync(null);
      }
    } finally {
      setBusy(null);
    }
  };

  const now = Date.now();

  return (
    <div className="space-y-3 min-w-0">
      <SettingsHeaderAction>
        <Button variant="primary" size="sm" onClick={() => setCreating(true)}>
          <PlusIcon size={13} />
          新增定价
        </Button>
      </SettingsHeaderAction>

      {error !== null && <Banner tone="danger" icon={<AlertCircleIcon size={14} />} title={error} />}
      {notice !== null && <Banner tone="success" icon={<CheckIcon size={14} />} title={notice} />}

      {/* 自动同步：开关加真实任务状态。 */}
      <Panel className="px-3.5 py-3 space-y-1.5">
        <ToggleField
          label="models.dev 费率自动同步"
          description={autoSyncStatusText(autoSync, now)}
          control={
            <div className="flex items-center gap-2">
              <Switch
                checked={autoSync?.enabled === true}
                disabled={busy !== null || autoSync === null}
                label="models.dev 费率自动同步"
                onChange={(next) => void toggleAutoSync(next)}
              />
              <Button
                variant="outline"
                size="sm"
                disabled={busy !== null}
                onClick={() => void runSync()}
              >
                <RefreshIcon size={13} />
                {busy === 'sync' ? '同步中' : '立即同步'}
              </Button>
            </div>
          }
        />
        {autoSync?.jobExists === true && (
          <Text tone="secondary" size="2xs" className="block">
            系统任务 · {scheduleText(autoSync.schedule)} · 下次{' '}
            {autoSync.nextRunAtMs === null || autoSync.nextRunAtMs === undefined
              ? '未安排'
              : formatRuleTime(autoSync.nextRunAtMs)}
          </Text>
        )}
        {autoSync?.lastRunSummary !== null && autoSync?.lastRunSummary !== undefined && (
          <Text tone="danger" size="2xs" className="block">
            上次失败原因：{autoSync.lastRunSummary}
          </Text>
        )}
      </Panel>

      {/* 规则表 */}
      <Panel className="flex flex-col min-w-0">
        <PanelHeader
          title="价格规则"
          description="单价按美元 / 百万 Token；同优先级下匹配式更具体的先命中"
          action={
            <Button
              variant="outline"
              size="sm"
              disabled={busy !== null}
              onClick={() => void runRecalculate()}
            >
              {busy === 'recalculate' ? '重算中' : '重算历史成本'}
            </Button>
          }
        />

        <div className="flex items-center gap-3 px-3.5 pt-2.5 pb-2">
          <div className="w-[240px] shrink-0">
            <Input
              icon={<SearchIcon size={13} />}
              value={query}
              placeholder="搜索模型匹配式"
              onChange={(event) => {
                setQuery(event.target.value);
                setPage(1);
              }}
              onClear={() => {
                setQuery('');
                setPage(1);
              }}
            />
          </div>
          <Text tone="secondary" size="xs">
            模型定价 · {filtered.length}
          </Text>
        </div>

        <div className="flex-1 min-w-0 px-3.5 pb-3">
          {rules === null ? (
            <LoadingState label="正在读取价格规则" />
          ) : filtered.length === 0 ? (
            <EmptyState
              title={rules.length === 0 ? '还没有任何价格规则' : '没有匹配的规则'}
              description={
                rules.length === 0
                  ? '可以从 models.dev 拉取常用模型费率，或点右上角「新增定价」手工添加一条'
                  : '换一个匹配式关键词再试'
              }
              action={
                rules.length === 0 ? (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy !== null}
                    onClick={() => void runSync()}
                  >
                    <RefreshIcon size={13} />
                    从 models.dev 同步
                  </Button>
                ) : undefined
              }
            />
          ) : (
            <table className="w-full table-fixed border-separate border-spacing-0">
              <thead>
                <tr>
                  <th
                    scope="col"
                    className="sticky top-0 z-10 bg-surface h-8 px-2.5 text-left border-b border-border-hairline"
                  >
                    <Text tone="secondary" size="xs" strong>
                      模型匹配式
                    </Text>
                  </th>
                  <th
                    scope="col"
                    className="sticky top-0 z-10 bg-surface h-8 px-2.5 text-left w-[88px] border-b border-border-hairline"
                  >
                    <Text tone="secondary" size="xs" strong>
                      计费风格
                    </Text>
                  </th>
                  {['输入', '输出'].map((label) => (
                    <th
                      key={label}
                      scope="col"
                      className="sticky top-0 z-10 bg-surface h-8 px-2.5 text-right w-[90px] border-b border-border-hairline"
                    >
                      <Text tone="secondary" size="xs" strong>
                        {label}
                      </Text>
                    </th>
                  ))}
                  <th
                    scope="col"
                    className="sticky top-0 z-10 bg-surface h-8 px-2.5 text-right w-[116px] border-b border-border-hairline"
                  >
                    <Text tone="secondary" size="xs" strong>
                      缓存 读/写
                    </Text>
                  </th>
                  <th
                    scope="col"
                    className="sticky top-0 z-10 bg-surface h-8 px-2.5 text-right w-[64px] border-b border-border-hairline"
                  >
                    <Text tone="secondary" size="xs" strong>
                      倍率
                    </Text>
                  </th>
                  <th
                    scope="col"
                    className="sticky top-0 z-10 bg-surface h-8 px-1.5 text-right w-[72px] border-b border-border-hairline"
                  >
                    <Text tone="secondary" size="xs" strong>
                      操作
                    </Text>
                  </th>
                </tr>
              </thead>
              <tbody>
                {visible.map((rule) => (
                  <tr key={rule.id} className="h-10 hover:bg-hover">
                    <td className="px-2.5 align-middle min-w-0">
                      <Text size="row" truncate title={rule.modelPattern}>
                        {rule.modelPattern}
                      </Text>
                      <Text tone="tertiary" size="2xs" truncate className="block">
                        优先级 {rule.priority} · {formatRuleTime(rule.updatedAtMs)}
                      </Text>
                    </td>
                    <td className="px-2.5 align-middle whitespace-nowrap">
                      <Text tone="secondary" size="row">
                        {pricingStyleLabel(rule.pricingStyle, true)}
                      </Text>
                    </td>
                    <td className="px-2.5 align-middle text-right whitespace-nowrap">
                      <Text size="row" mono>
                        {formatPriceCellFixed(rule.inputPricePerMMicros)}
                      </Text>
                    </td>
                    <td className="px-2.5 align-middle text-right whitespace-nowrap">
                      <Text size="row" mono>
                        {formatPriceCellFixed(rule.outputPricePerMMicros)}
                      </Text>
                    </td>
                    <td className="px-2.5 align-middle text-right whitespace-nowrap">
                      <Text size="row" mono className="block leading-5">
                        {formatPriceCellFixed(rule.cacheReadPricePerMMicros)}
                      </Text>
                      <Text tone="tertiary" size="2xs" className="block leading-4">
                        写 {formatPriceCellFixed(rule.cacheWritePricePerMMicros)}
                      </Text>
                    </td>
                    <td className="px-2.5 align-middle text-right whitespace-nowrap">
                      <Text tone="secondary" size="row" mono>
                        {formatMultiplier(rule.multiplierBasisPoints)}×
                      </Text>
                    </td>
                    <td className="px-1.5 align-middle">
                      <div className="flex items-center justify-end gap-0.5">
                        <IconButton
                          label={`编辑 ${rule.modelPattern}`}
                          onClick={() => setEditing(rule)}
                        >
                          <EditIcon size={13} />
                        </IconButton>
                        <IconButton
                          label={`删除 ${rule.modelPattern}`}
                          onClick={() => setRemoving(rule)}
                        >
                          <TrashIcon size={13} />
                        </IconButton>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        {/* 一条规则都没有时不画分页条：空态已经把话说完，再摆一排禁用页码只是噪音。 */}
        {filtered.length > 0 && (
          <Pagination
            label="价格规则分页"
            page={current}
            pageCount={pageCount}
            pageSize={pageSize}
            pageSizeOptions={PAGE_SIZES}
            onPageChange={setPage}
            onPageSizeChange={(size) => {
              setPageSize(size);
              setPage(1);
            }}
            summary={`第 ${(current - 1) * pageSize + 1} 至 ${(current - 1) * pageSize + visible.length} 条，共 ${filtered.length} 条`}
          />
        )}
      </Panel>

      <Text tone="secondary" size="xs" className="block pb-1">
        单价的单位是美元 / 百万 Token。命中规则按四级顺序判定：优先级大的优先，其次匹配式里
        字面字符更多的优先，再次更新时间更晚的优先，最后按规则 ID 排序。改动规则不会自动
        改写已经落库的历史成本，需要点一次「重算历史成本」。
      </Text>

      <PricingRuleDialog
        open={creating || editing !== null}
        onClose={() => {
          setCreating(false);
          setEditing(null);
        }}
        rule={editing ?? undefined}
        onSubmit={(input) => applyRule(input, editing)}
      />

      <Dialog
        open={removing !== null}
        onClose={() => setRemoving(null)}
        title="删除定价"
        size="sm"
        secondary={{ label: '取消', onClick: () => setRemoving(null) }}
        primary={{
          label: '删除',
          onClick: () => {
            if (removing !== null) void removeRule(removing);
          },
        }}
      >
        <Text size="row" tone="secondary" className="block whitespace-pre-wrap break-words">
          {`确定删除规则 ${removing?.modelPattern ?? ''} 吗？\n删除后命中它的请求会回到「未定价」，已落库的历史成本保持原值，需要重算才会更新。`}
        </Text>
      </Dialog>
    </div>
  );
};
