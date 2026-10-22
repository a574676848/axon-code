import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, CheckIcon, SettingsIcon, SpinnerIcon } from '../icons';
import type { GatewayHealth } from '../types';
import * as ipc from '../ipc';
import { Button } from './ui/controls';
import { Banner, Text } from './ui/surfaces';
import { Dialog } from './ui/overlays';

/*
 * 网关断连的统一呈现。
 *
 * 修复动作由后端按故障类型给出（缺服务 / 未运行 / 未监听 / 凭据问题 / 版本过旧），
 * 前端只负责按键名分发，因此新增故障类型不用改这里。
 * 提权被取消或失败时如实展示失败原因，绝不假装已恢复。
 *
 * 弹窗与主区共用同一份面板与同一份修复状态：断连是一个事实，
 * 不该因为「从哪一扇门看进去」而有第二套说法。两处只换布局（居中页 / 弹窗正文）。
 */

/** 探查间隔，未连接时轮询以自动感知恢复。 */
const POLL_INTERVAL_MS = 4000;

/** 失败原因的最大展示长度；再长就截断，避免撑破弹窗。 */
const FAILURE_DETAIL_MAX_CHARS = 160;

/** 折叠换行并截断失败原因。 */
function truncateDetail(detail: string): string {
  const compact = detail.split(/\s+/).join(' ').trim();
  return compact.length > FAILURE_DETAIL_MAX_CHARS
    ? `${compact.slice(0, FAILURE_DETAIL_MAX_CHARS)}…`
    : compact;
}

/** 修复动作的可用性与执行状态。 */
interface GatewayRepair {
  /** 正在执行的动作名；为空表示空闲。 */
  busy: string | null;
  error: string | null;
  notice: string | null;
  /** 后端推荐的首要动作。 */
  recommended: string | undefined;
  /** 其余动作。 */
  additional: string[];
  /** 执行某一个修复动作。 */
  run: (action: string) => void;
}

/**
 * 修复动作的执行器：两处面板共用一份，失败与回执也只存一份。
 * @param health 探测结果。
 * @param onRetry 重新探测。
 * @param onOpenSettings 打开设置（配置网关密钥）。
 */
function useGatewayRepair(
  health: GatewayHealth,
  onRetry: () => void | Promise<void>,
  onOpenSettings: () => void,
): GatewayRepair {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const runService = useCallback(
    async (mode: 'start' | 'install' | 'restart', label: string) => {
      setBusy(label);
      setError(null);
      setNotice(null);
      try {
        const result = await ipc.gatewayStartService(mode);
        if (result.success) {
          setNotice(result.message);
        } else {
          setError(`${result.message}${result.output.length > 0 ? `：${result.output.trim()}` : ''}`);
        }
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : '服务操作失败');
      } finally {
        setBusy(null);
        await onRetry();
      }
    },
    [onRetry],
  );

  const run = useCallback(
    (action: string) => {
      const dispatch = async (): Promise<void> => {
        if (action === '配置网关密钥') {
          onOpenSettings();
          return;
        }
        if (action === '启动服务') return runService('start', action);
        if (action === '注册服务') return runService('install', action);
        if (action === '重启服务' || action === '升级后台服务') {
          return runService('restart', action);
        }
        if (action === '打开配置文件' || action === '打开日志目录') {
          try {
            await ipc.openDocument(action === '打开配置文件' ? 'config' : 'logs');
          } catch (failure) {
            setError(failure instanceof Error ? failure.message : `${action}失败`);
          }
          return;
        }
        if (action === '重试') {
          setBusy(action);
          try {
            await onRetry();
          } finally {
            setBusy(null);
          }
          return;
        }
        // 未知动作不静默忽略：后端新增了动作而前端还没跟上时，要看得见。
        setError(`暂不支持的修复动作：${action}`);
      };
      void dispatch();
    },
    [onOpenSettings, onRetry, runService],
  );

  const recommended = health.actions.find((action) => action !== '重试');
  const additional = health.actions.filter(
    (action) => action !== recommended && action !== '重试',
  );

  return { busy, error, notice, recommended, additional, run };
}

/** 面板内容的公共入参。 */
interface FailureBodyProps {
  health: GatewayHealth;
  detail: string | null;
  repair: GatewayRepair;
  /** 布局：居中页自画标题，弹窗里标题由弹窗自己的标题栏承担。 */
  layout: 'page' | 'dialog';
  /** 底部的操作区。 */
  actions: React.ReactNode;
}

/**
 * 断连面板：图标 + 结论 + 网关地址 + 服务状态 + 失败原因 + 回执。
 * 两种布局只差对齐方式与标题归属，内容与排版阶梯完全一致。
 */
const FailureBody: React.FC<FailureBodyProps> = ({ health, detail, repair, layout, actions }) => {
  const page = layout === 'page';
  return (
    <div
      className={
        page
          ? 'flex flex-col items-center gap-2.5 px-6 text-center'
          : 'flex flex-col items-start gap-2.5'
      }
    >
      <span className="flex h-14 w-14 shrink-0 items-center justify-center rounded-md bg-danger-soft text-danger">
        <AlertCircleIcon size={36} />
      </span>
      {page && (
        <Text size="body" strong className="block">
          未连接到 Axon 后台服务
        </Text>
      )}
      <Text
        tone="secondary"
        size="meta"
        className={`block max-w-lg leading-5 ${page ? '' : 'text-left'}`}
      >
        {health.reason}
      </Text>
      <span className="block max-w-lg break-all font-mono text-[11px] leading-4 text-text-tertiary">
        网关地址 {health.target.length > 0 ? health.target : '未配置'}
      </span>

      {repair.notice !== null && (
        <div className="w-full">
          <Banner tone="success" icon={<CheckIcon size={14} />} title={repair.notice} />
        </div>
      )}
      {repair.error !== null && (
        <div className="w-full">
          <Banner tone="danger" icon={<AlertCircleIcon size={14} />} title={repair.error} />
        </div>
      )}

      <div className={`flex flex-wrap items-center gap-2 ${page ? 'justify-center pt-1' : ''}`}>
        {actions}
      </div>

      <span className="block text-[11px] leading-4 text-text-tertiary">
        服务状态：{health.responding ? '网关有响应' : '网关无响应'} ·{' '}
        {health.listening ? '端口已监听' : '端口未监听'}
      </span>
      {/* 拦截器带来的具体失败原因：探测结论说明「怎么了」，这一行说明「哪一次」。 */}
      {detail !== null && detail.trim().length > 0 && (
        <span className="block max-w-lg break-all font-mono text-[11px] leading-4 text-text-tertiary">
          最近一次失败：{truncateDetail(detail)}
        </span>
      )}
    </div>
  );
};

/** 面板上的次要动作：后端给出的其余修复途径。 */
const AdditionalActions: React.FC<{ repair: GatewayRepair }> = ({ repair }) => (
  <>
    {repair.additional.map((action) => (
      <Button
        key={action}
        variant="ghost"
        size="sm"
        disabled={repair.busy !== null}
        onClick={() => repair.run(action)}
      >
        {repair.busy === action ? <SpinnerIcon size={14} className="animate-spin" /> : null}
        {repair.busy === action ? `${action}中` : action}
      </Button>
    ))}
  </>
);

interface GatewayUnconnectedProps {
  /** 探测结果。 */
  health: GatewayHealth;
  /** 重新探测。 */
  onRetry: () => void | Promise<void>;
  open: boolean;
  onClose: () => void;
  onOpenSettings: () => void;
  /** 触发这次判定的那次调用失败原因；没有拦截器证据时为空。 */
  detail?: string | null;
}

export const GatewayUnconnected: React.FC<GatewayUnconnectedProps> = ({
  health, onRetry, open, onClose, onOpenSettings, detail = null,
}) => {
  const repair = useGatewayRepair(health, onRetry, onOpenSettings);
  const primary = repair.recommended ?? '重试';

  // 未连接时自动轮询，服务被外部修好后界面自行恢复。
  useEffect(() => {
    const timer = window.setInterval(() => void onRetry(), POLL_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [onRetry]);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="未连接到 Axon 后台服务"
      primary={{
        label: repair.busy === null ? primary : `${repair.busy}中`,
        disabled: repair.busy !== null,
        icon:
          repair.busy !== null ? (
            <SpinnerIcon size={14} className="animate-spin" />
          ) : (
            <SettingsIcon size={14} />
          ),
        onClick: () => repair.run(primary),
      }}
      secondary={
        repair.recommended === undefined
          ? undefined
          : {
              label: repair.busy === '重试' ? '正在重试' : '重试连接',
              disabled: repair.busy !== null,
              onClick: () => repair.run('重试'),
            }
      }
    >
      <FailureBody
        health={health}
        detail={detail}
        repair={repair}
        layout="dialog"
        actions={<AdditionalActions repair={repair} />}
      />
    </Dialog>
  );
};

interface GatewayUnavailableStateProps {
  /** 探测结果。 */
  health: GatewayHealth;
  /** 触发这次判定的那次调用失败原因；没有拦截器证据时为空。 */
  detail?: string | null;
  /** 重新探测。 */
  onRetry: () => void | Promise<void>;
  /** 打开设置里的远控分区，便于配置网关密钥。 */
  onOpenSettings: () => void;
}

/**
 * 服务不可用时主区域的常驻状态。
 *
 * 弹窗可能被关掉，关掉之后主区域不能是一片空白：用户需要知道「现在用不了、
 * 为什么、去哪里按」，并且不必等下一次心跳才有反馈。
 */
export const GatewayUnavailableState: React.FC<GatewayUnavailableStateProps> = ({
  health,
  detail = null,
  onRetry,
  onOpenSettings,
}) => {
  const repair = useGatewayRepair(health, onRetry, onOpenSettings);
  return (
    <div className="flex-1 min-h-0 h-full overflow-y-auto scroll-visible">
      {/* 内容短时居中，内容长时从顶部开始滚：不用 justify-center 直接裁剪顶部。 */}
      <div className="min-h-full flex flex-col items-center justify-center gap-3 px-6 py-8 text-center">
        <FailureBody
          health={health}
          detail={detail}
          repair={repair}
          layout="page"
          actions={
            <>
              <Button
                variant="primary"
                size="md"
                disabled={repair.busy !== null}
                onClick={() => repair.run('重试')}
              >
                {repair.busy !== null ? <SpinnerIcon size={14} className="animate-spin" /> : null}
                {repair.busy !== null ? '正在重试' : '重试连接'}
              </Button>
              <Button variant="outline" size="md" onClick={onOpenSettings}>
                打开设置
              </Button>
              <AdditionalActions repair={repair} />
            </>
          }
        />
        <Text tone="tertiary" size="meta" className="block max-w-lg leading-5">
          侧栏的工作区与会话列表仍可查看；服务恢复后这里会自动接回来。
        </Text>
      </div>
    </div>
  );
};
