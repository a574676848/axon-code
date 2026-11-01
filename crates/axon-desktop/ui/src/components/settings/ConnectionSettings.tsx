import React, { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AlertCircleIcon,
  CheckIcon,
  PlusIcon,
  RefreshIcon,
  RetryIcon,
  SpinnerIcon,
} from '../../icons';
import type { ProfileView } from '../../connectionTypes';
import type { SystemInfo } from '../../types';
import * as ipc from '../../ipc';
import {
  deleteProfile,
  ensureConnectionLoaded,
  loadConnectionState,
  retryProbe,
  switchProfile,
  useConnectionFact,
} from '../../connectionState';
import { Button } from '../ui/controls';
import {
  Banner,
  Chip,
  LoadingState,
  Panel,
  SectionHeading,
  StateDot,
  Text,
} from '../ui/surfaces';
import { CardList, EntityCard } from '../ui/cards';
import { daemonHealthView, daemonStateView } from './runtimeStatus';
import {
  CapabilitySummary,
  ConnectionFactRow,
  ConnectionProfileDialog,
} from './ConnectionProfileDialog';
import { KeyManagementCard } from './KeyManagementCard';

/*
 * 设置 › 连接：先看清「此刻连的是谁」，再管理「还能连谁」。两块的顺序不可颠倒。
 *
 * 一个事实源：这里读的是 connectionState 的那一份生效档案与健康结论，不另发探测、
 * 不在本地推导健康态，也不缓存第二份结论。
 *
 * 未连接态不退化：后端版本、往返延迟与能力摘要一律显示「未知」，绝不显示 0 或
 * 伪造值；能力相关入口可见但置灰，点击给「未连接」，也绝不报「后台服务版本过旧」
 * （那是「已连上但后端确实没有 /meta」才成立的结论）。
 *
 * 本机后台服务的租约与排队读数只在非云端档案下渲染：这些读数来自本机运行态，
 * 挂在云端档案下面只会张冠李戴。云端档案的 service_control 被后端按模式覆盖为
 * false，因此重启入口整块不渲染。
 */

/** 本机档案的 id 由后端固定为 `local`（档案 id 不可变），据此区分地址由配置决定的档案。 */
const LOCAL_PROFILE_ID = 'local';

/** 一条操作回执：成功与失败共用同一个提示位，一个字都不改。 */
interface Notice {
  tone: 'success' | 'danger';
  text: string;
}

export const ConnectionSettings: React.FC = () => {
  const fact = useConnectionFact();
  const snapshot = fact.snapshot;

  const [daemonInfo, setDaemonInfo] = useState<SystemInfo | null>(null);
  const [daemonError, setDaemonError] = useState('');
  const [notice, setNotice] = useState<Notice | null>(null);
  const [retrying, setRetrying] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [query, setQuery] = useState('');
  const [dialogOpen, setDialogOpen] = useState(false);
  /** 正在查看配置的档案；为空表示新增。 */
  const [dialogProfile, setDialogProfile] = useState<ProfileView | null>(null);

  // 直接从一个入口打开设置时也要有结论：这一条是幂等的，外壳读过就不会重发。
  useEffect(() => {
    ensureConnectionLoaded();
  }, []);

  /** 本机后台读数与本机档案同源；云端档案下不读也不显示。 */
  const showDaemon = snapshot !== null && !snapshot.isCloud;

  const loadDaemon = useCallback(async () => {
    try {
      const info = await ipc.systemInfo();
      setDaemonInfo(info);
      setDaemonError('');
    } catch (failure: unknown) {
      setDaemonInfo(null);
      setDaemonError(failure instanceof Error ? failure.message : '读取本机后台服务状态失败');
    }
  }, []);

  useEffect(() => {
    if (showDaemon) void loadDaemon();
  }, [showDaemon, loadDaemon]);

  /** 重试连接：只发一次探测，成功与否以快照为准，不在本地猜。 */
  const retry = async () => {
    setRetrying(true);
    setNotice(null);
    try {
      await retryProbe();
    } finally {
      setRetrying(false);
      if (showDaemon) await loadDaemon();
    }
  };

  /**
   * 重启后台服务：只有 service_control 为真时才真正执行。
   *
   * 未连接时这个入口可见但置灰，点击只会得到「未连接」的结论，不会去打一个连不上的端点。
   */
  const restart = async () => {
    if (snapshot === null) return;
    const enabled =
      snapshot.capsKnown &&
      snapshot.capabilities.some((item) => item.key === 'service_control' && item.enabled);
    if (!enabled) {
      setNotice({ tone: 'danger', text: '未连接：无法执行后台服务操作，请先重试连接。' });
      return;
    }
    setRestarting(true);
    setNotice(null);
    try {
      const result = await ipc.gatewayStartService('restart');
      if (result.success) {
        setNotice({ tone: 'success', text: result.message });
      } else {
        const output = result.output.trim();
        setNotice({
          tone: 'danger',
          text: output.length > 0 ? `${result.message}：${output}` : result.message,
        });
      }
    } catch (failure: unknown) {
      setNotice({
        tone: 'danger',
        text: failure instanceof Error ? failure.message : '重启后台服务失败',
      });
    } finally {
      setRestarting(false);
      await loadDaemon();
      await retryProbe();
    }
  };

  const profiles = snapshot?.profiles ?? [];

  const visibleProfiles = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (keyword.length === 0) return profiles;
    return profiles.filter(
      (profile) =>
        profile.label.toLowerCase().includes(keyword) ||
        profile.mode.toLowerCase().includes(keyword) ||
        profile.endpoint.toLowerCase().includes(keyword),
    );
  }, [profiles, query]);

  /** 切换档案：后端以「重启应用」实现，没进重启流程就把原因原样抛给弹窗。 */
  const switchTo = async (id: string): Promise<void> => {
    const outcome = await switchProfile(id);
    if (!outcome.started) throw new Error(outcome.message);
  };

  /** 删除档案：生效中的档案不可删，后端也会拒绝；界面提前拦一道并说明原因。 */
  const remove = async (id: string): Promise<void> => {
    await deleteProfile(id);
  };

  if (snapshot === null) {
    return (
      <div className="space-y-3">
        {fact.error === null ? (
          <LoadingState label="正在读取连接状态" />
        ) : (
          <Banner
            tone="danger"
            icon={<AlertCircleIcon size={13} />}
            title={`读取连接状态失败：${fact.error}`}
            action={
              <Button variant="ghost" size="sm" onClick={() => void loadConnectionState()}>
                重试
              </Button>
            }
          />
        )}
      </div>
    );
  }

  const serviceControlEnabled =
    snapshot.capsKnown &&
    snapshot.capabilities.some((item) => item.key === 'service_control' && item.enabled);
  /** 已知且为假时隐藏（后端的确定事实），尚未取到时可见但置灰。 */
  const showRestart = snapshot.capsKnown ? serviceControlEnabled : true;

  const daemon = daemonStateView(daemonInfo?.daemon_state);
  const daemonHealth = daemonHealthView(
    daemonInfo?.daemon_health,
    daemonInfo === null ? daemonError : daemonInfo.daemon_health_error,
  );
  const daemonDetail =
    daemonInfo?.daemon_detail ?? (daemonError.length > 0 ? daemonError : '正在读取');

  const dialogId = dialogProfile?.id ?? '';
  const dialogIsActive = dialogProfile !== null && dialogProfile.active;
  const dialogIsLocal = dialogProfile !== null && dialogProfile.id === LOCAL_PROFILE_ID;

  return (
    <div className="space-y-4">
      {fact.error !== null && (
        <Banner
          tone="danger"
          icon={<AlertCircleIcon size={13} />}
          title={`读取连接状态失败：${fact.error}`}
          action={
            <Button variant="ghost" size="sm" onClick={() => void loadConnectionState()}>
              重试
            </Button>
          }
        />
      )}

      {/*
        启动期提示：状态文件损坏、来自更高的结构版本、或刚刚完成迁移。
        这三件事都属于「本次只读或已换过结构」，必须让用户看见一次，
        而不是只在日志里留一行。它是只读事实，因此不配动作按钮。
      */}
      {snapshot.notice !== null && (
        <Banner
          tone="danger"
          icon={<AlertCircleIcon size={13} />}
          title={snapshot.notice}
        />
      )}

      {/* ------------------------------------------------------------ 当前连接 */}
      <Panel clip>
        <div className="px-3.5 py-3 hairline-b border-border-hairline">
          <div className="flex items-center gap-2 min-w-0">
            <Text size="sm" strong truncate className="min-w-0">
              {snapshot.activeLabel}
            </Text>
            <Chip>{snapshot.mode}</Chip>
            {snapshot.readOnly && <Chip tone="warning">只读运行</Chip>}
          </div>
          <Text tone="secondary" size="xs" className="block mt-0.5">
            当前连接的后端与它自报的能力
          </Text>
        </div>

        <div className="px-3.5 py-3 space-y-2">
          <ConnectionFactRow
            label="后端地址"
            value={
              <Text size="xs" mono truncate title={snapshot.endpoint}>
                {snapshot.endpoint}
              </Text>
            }
          />
          <ConnectionFactRow
            label="后端版本"
            value={
              <span className="flex items-center gap-1.5 min-w-0">
                <Text size="xs" truncate className="min-w-0">
                  {snapshot.capsKnown ? (snapshot.backendVersion ?? '未知') : '未知'}
                </Text>
                {snapshot.capsKnown && snapshot.backendOutdated && (
                  <Chip tone="warning">后台服务版本过旧</Chip>
                )}
              </span>
            }
          />
          <ConnectionFactRow
            label="连接状态"
            value={
              <span className="flex items-start gap-1.5 min-w-0">
                <span className="shrink-0 mt-0.5">
                  <StateDot state={snapshot.health.dot} />
                </span>
                <span className="min-w-0">
                  <Text size="xs" className="block">
                    {snapshot.health.label}
                  </Text>
                  <Text tone="secondary" size="xs" className="block">
                    {snapshot.health.detail}
                  </Text>
                </span>
              </span>
            }
          />
          <ConnectionFactRow
            label="往返延迟"
            value={
              <span className="flex items-center gap-2 min-w-0">
                <Text size="xs" className="tabular-nums">
                  {snapshot.capsKnown && snapshot.latencyMs !== null
                    ? `${snapshot.latencyMs} 毫秒`
                    : '未知'}
                </Text>
                {!snapshot.clockVerified && <Chip tone="warning">时钟未校验</Chip>}
              </span>
            }
          />
          <ConnectionFactRow
            label="能力摘要"
            value={<CapabilitySummary capabilities={snapshot.capabilities} known={snapshot.capsKnown} />}
          />
        </div>

        {showDaemon && (
          <div className="px-3.5 py-3 hairline-t border-border-hairline space-y-1.5">
            <div className="flex items-center justify-between gap-3">
              <Text size="sm">本机后台服务</Text>
              <span className="inline-flex items-center gap-1.5 shrink-0">
                <StateDot state={daemon.dot} />
                <Text size="xs" tone={daemon.attention ? 'danger' : 'primary'}>
                  {daemon.label}
                </Text>
              </span>
            </div>
            <Text tone="secondary" size="xs" className="block">
              {daemonDetail}
            </Text>
            <Text tone={daemonHealth.warning ? 'danger' : 'secondary'} size="xs" className="block">
              {daemonHealth.text}
            </Text>
          </div>
        )}

        <div className="px-3.5 py-3 hairline-t border-border-hairline flex flex-wrap items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={retrying || fact.busy}
            onClick={() => void retry()}
          >
            {retrying || fact.busy ? (
              <SpinnerIcon size={13} className="state-spinner" />
            ) : (
              <RefreshIcon size={13} />
            )}
            {retrying || fact.busy ? '正在探测' : '重试连接'}
          </Button>
          {showRestart && (
            <Button
              variant="outline"
              size="sm"
              aria-disabled={!serviceControlEnabled}
              className={serviceControlEnabled ? undefined : 'text-text-tertiary'}
              disabled={restarting}
              onClick={() => void restart()}
            >
              {restarting ? (
                <SpinnerIcon size={13} className="state-spinner" />
              ) : (
                <RetryIcon size={13} />
              )}
              {restarting ? '正在重启' : '重启后台服务'}
            </Button>
          )}
        </div>

        {notice !== null && (
          <div className="px-3.5 pb-3">
            <Banner
              tone={notice.tone}
              icon={
                notice.tone === 'success' ? <CheckIcon size={13} /> : <AlertCircleIcon size={13} />
              }
              title={notice.text}
              action={
                <Button variant="ghost" size="sm" onClick={() => setNotice(null)}>
                  知道了
                </Button>
              }
            />
          </div>
        )}

        {snapshot.plaintextPrivate && (
          <div className="px-3.5 pb-3">
            <Banner
              tone="danger"
              icon={<AlertCircleIcon size={13} />}
              title="凭据将以明文过网"
              action={
                <Text tone="secondary" size="xs" className="block">
                  当前档案使用明文传输，凭据以明文过网。
                </Text>
              }
            />
          </div>
        )}
      </Panel>

      {/* ------------------------------------------------------------ 档案清单 */}
      <div className="space-y-2">
        <SectionHeading
          title="档案清单"
          description="同一时刻只有一份档案生效，切换档案会重启应用"
          action={
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setDialogProfile(null);
                setDialogOpen(true);
              }}
            >
              <PlusIcon size={13} />
              新增档案
            </Button>
          }
        />

        {!snapshot.secretStoreAvailable && (
          <Banner
            tone="danger"
            icon={<AlertCircleIcon size={13} />}
            title={snapshot.secretStoreReason ?? '系统凭据库不可用'}
            action={
              <Text tone="secondary" size="xs" className="block">
                云档案凭据只在本次会话有效，应用重启后需要重新填写。
              </Text>
            }
          />
        )}

        <CardList
          query={query}
          onQueryChange={setQuery}
          placeholder="搜索档案名称、模式或地址"
          summary={`共 ${visibleProfiles.length} 份档案`}
          emptyTitle={query.trim().length > 0 ? '没有匹配的档案' : '还没有任何档案'}
          emptyDescription={
            query.trim().length > 0
              ? '换一个关键词，或清空搜索框查看全部'
              : '点右上角「新增档案」接入一台后端'
          }
        >
          {visibleProfiles.map((profile) => (
            <EntityCard
              key={profile.id}
              title={profile.label}
              badges={
                <span className="flex items-center gap-1.5">
                  <Chip>{profile.mode}</Chip>
                  {profile.active && <Chip tone="accent">生效中</Chip>}
                </span>
              }
              description={
                profile.active
                  ? '当前生效档案，切换或删除都要先换到别的档案'
                  : '未生效，切换后才会连上这台后端'
              }
              locator={profile.endpoint}
              metaNode={
                <span className="flex items-center gap-1.5 min-w-0">
                  <StateDot state={profile.healthDot} />
                  <Text tone="secondary" size="micro" truncate className="min-w-0">
                    {profile.healthLabel}
                  </Text>
                  <Text tone="tertiary" size="micro" truncate className="min-w-0">
                    {profile.credential}
                  </Text>
                </span>
              }
              onOpen={() => {
                setDialogProfile(profile);
                setDialogOpen(true);
              }}
              inspectLabel="查看配置"
              onInspect={() => {
                setDialogProfile(profile);
                setDialogOpen(true);
              }}
            />
          ))}
        </CardList>
      </div>

      {/* ------------------------------------------------------------ 密钥管理 */}
      {/*
        密钥管理卡片只在本次凭据是超管时渲染：普通 Key 连接时整块不渲染，既不置灰也不留
        占位。判断依据是后端 /meta 自报的 superAdmin，界面不自己推断身份。
      */}
      {snapshot.keyManagementVisible && <KeyManagementCard />}

      <ConnectionProfileDialog
        open={dialogOpen}
        title={dialogProfile === null ? '新增档案' : `档案配置 · ${dialogProfile.label}`}
        initial={
          dialogProfile === null
            ? undefined
            : { label: dialogProfile.label, address: dialogProfile.endpoint }
        }
        submitLabel="保存档案"
        readOnlyReason={
          dialogIsLocal
            ? '本机档案的后端地址由「远控」分区里的网关配置决定，不能在这里修改。'
            : undefined
        }
        onClose={() => setDialogOpen(false)}
        onSaved={() => setDialogOpen(false)}
        onSwitch={dialogProfile !== null && !dialogIsActive ? () => switchTo(dialogId) : undefined}
        switchBlockedReason={dialogIsActive ? '这已经是当前生效的档案' : undefined}
        onDelete={
          dialogProfile !== null && !dialogIsActive && !dialogIsLocal
            ? () => remove(dialogId)
            : undefined
        }
        deleteBlockedReason={
          dialogIsActive
            ? '生效中的档案不可删除，请先切换到别的档案'
            : dialogIsLocal
              ? '本机档案是数据的固定归属，不能删除'
              : undefined
        }
      />
    </div>
  );
};
