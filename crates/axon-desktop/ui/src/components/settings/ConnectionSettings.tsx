import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, PlusIcon, RefreshIcon, RetryIcon, SpinnerIcon } from '../../icons';
import type { ProfileView } from '../../connectionTypes';
import * as ipc from '../../ipc';
import {
  deleteProfile,
  ensureConnectionLoaded,
  latencyText,
  loadConnectionState,
  retryProbe,
  switchProfile,
  useConnectionFact,
} from '../../connectionState';
import { Button } from '../ui/controls';
import { Banner, Chip, LoadingState, SectionHeading, StateDot, Text } from '../ui/surfaces';
import { CardList, EntityCard } from '../ui/cards';
import { ConnectionProfileDialog } from './ConnectionProfileDialog';
import { KeyManagementCard } from './KeyManagementCard';

/*
 * 设置 › 连接：只有一件事，管理能连哪些后端。
 *
 * 页面从简到只有三块：档案卡片、新增档案、超管身份下的密钥管理。曾经的「当前连接」
 * 大卡（后端地址、版本、连接状态、能力摘要、租约读数）整块下线：当前连的是谁、连得
 * 好不好，都由生效档案卡自己回答（状态点、健康结论、延迟），不再占一屏重复读数。
 *
 * 一个事实源：这里读的是 connectionState 的那一份生效档案与健康结论，不另发探测、
 * 不在本地推导健康态，也不缓存第二份结论。未取到延迟时写「未知」，绝不写 0。
 *
 * 生效档案卡上的两个动作按档案模式决定：重新连接两种档案都给；重启后台服务只给
 * 本机档案，因为云档案的 service_control 恒为 false，而且那条路走的是本机 CLI。
 */

/** 一条操作回执：成功与失败共用同一个提示位。 */
interface Notice {
  tone: 'success' | 'danger';
  text: string;
}

/** 本机档案的 id 由后端固定为 `local`（档案 id 不可变），据此区分地址由配置决定的档案。 */
const LOCAL_PROFILE_ID = 'local';

export const ConnectionSettings: React.FC = () => {
  const fact = useConnectionFact();
  const snapshot = fact.snapshot;

  const [notice, setNotice] = useState<Notice | null>(null);
  const [retrying, setRetrying] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);
  /** 正在查看配置的档案；为空表示新增。 */
  const [dialogProfile, setDialogProfile] = useState<ProfileView | null>(null);

  // 直接从一个入口打开设置时也要有结论：这一条是幂等的，外壳读过就不会重发。
  useEffect(() => {
    ensureConnectionLoaded();
  }, []);

  /** 重新连接：只发一次探测，成功与否以快照为准，不在本地猜。 */
  const retry = useCallback(async () => {
    setRetrying(true);
    setNotice(null);
    try {
      await retryProbe();
    } finally {
      setRetrying(false);
    }
  }, []);

  /**
   * 重启后台服务：走本机 CLI，因此即使网关此刻是断的也照样能执行。
   *
   * 这也正是它存在的理由：需要重启的时候，通常就是连不上的时候，此时再按能力位
   * 把它置灰等于把恢复手段锁死。
   */
  const restart = useCallback(async () => {
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
      await retryProbe();
    }
  }, []);

  /** 切换档案：后端以「重启应用」实现，没进重启流程就把原因原样抛给弹窗。 */
  const switchTo = async (id: string): Promise<void> => {
    const outcome = await switchProfile(id);
    if (!outcome.started) throw new Error(outcome.message);
  };

  /** 删除档案：生效中的档案不可删，后端也会拒绝；界面提前拦一道并说明原因。 */
  const remove = async (id: string): Promise<void> => {
    await deleteProfile(id);
  };

  const openDialog = (profile: ProfileView | null) => {
    setDialogProfile(profile);
    setDialogOpen(true);
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

  const profiles = snapshot.profiles;
  /** 生效档案的延迟读数：没取到就写「未知」，绝不写 0。 */
  const latency = latencyText(snapshot.latencyMs);

  /** 生效档案卡上的动作组；只有一份档案生效，因此这份节点只挂在那一张卡上。 */
  const activeActions = (
    <>
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
        {retrying || fact.busy ? '正在重连' : '重新连接'}
      </Button>
      {!snapshot.isCloud && (
        <Button variant="outline" size="sm" disabled={restarting} onClick={() => void restart()}>
          {restarting ? (
            <SpinnerIcon size={13} className="state-spinner" />
          ) : (
            <RetryIcon size={13} />
          )}
          {restarting ? '正在重启' : '重启后台服务'}
        </Button>
      )}
    </>
  );

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
        <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={snapshot.notice} />
      )}

      {notice !== null && (
        <p
          role="status"
          className={`text-xs leading-5 ${notice.tone === 'danger' ? 'text-danger' : 'text-text-secondary'}`}
        >
          {notice.text}
        </p>
      )}

      <SectionHeading
        title="档案清单"
        action={
          <Button variant="outline" size="sm" onClick={() => openDialog(null)}>
            <PlusIcon size={13} />
            新增档案
          </Button>
        }
      />

      <CardList
        emptyTitle="还没有任何档案"
        emptyDescription="点右上角「新增档案」接入一台后端"
      >
        {profiles.map((profile) => (
          <EntityCard
            key={profile.id}
            title={profile.label}
            badges={
              <span className="flex items-center gap-1.5">
                <Chip>{profile.mode}</Chip>
                {profile.active && <Chip tone="accent">生效中</Chip>}
              </span>
            }
            locator={profile.endpoint}
            metaNode={
              <span className="flex items-center gap-1.5 min-w-0">
                <StateDot state={profile.healthDot} />
                <Text tone="secondary" size="micro" truncate className="min-w-0">
                  {profile.healthLabel}
                </Text>
                {profile.active && (
                  <Text tone="tertiary" size="micro" className="shrink-0 tabular-nums">
                    延迟 {latency ?? '未知'}
                  </Text>
                )}
              </span>
            }
            footerActions={profile.active ? activeActions : undefined}
            onOpen={() => openDialog(profile)}
            onInspect={() => openDialog(profile)}
          />
        ))}
      </CardList>

      {/* 密钥管理卡片只在本次凭据是超管时渲染：普通 Key 连接时整块不渲染，既不置灰也不留占位。 */}
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
          dialogProfile?.id === LOCAL_PROFILE_ID
            ? '本机档案的后端地址由「远控」分区里的网关配置决定，不能在这里修改。'
            : undefined
        }
        onClose={() => setDialogOpen(false)}
        onSaved={() => setDialogOpen(false)}
        onSwitch={
          dialogProfile !== null && !dialogProfile.active
            ? () => switchTo(dialogProfile.id)
            : undefined
        }
        switchBlockedReason={dialogProfile?.active === true ? '这已经是当前生效的档案' : undefined}
        onDelete={
          dialogProfile !== null && !dialogProfile.active && dialogProfile.id !== LOCAL_PROFILE_ID
            ? () => remove(dialogProfile.id)
            : undefined
        }
        deleteBlockedReason={
          dialogProfile?.active === true
            ? '生效中的档案不可删除，请先切换到别的档案'
            : dialogProfile?.id === LOCAL_PROFILE_ID
              ? '本机档案是数据的固定归属，不能删除'
              : undefined
        }
      />
    </div>
  );
};
