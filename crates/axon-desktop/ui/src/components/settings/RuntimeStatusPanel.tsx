import React, { useCallback, useEffect, useState } from 'react';
import { RetryIcon, SpinnerIcon } from '../../icons';
import type { SystemInfo } from '../../types';
import * as ipc from '../../ipc';
import { IconButton } from '../ui/controls';
import { ListRow, StateDot, Text } from '../ui/surfaces';
import { daemonHealthView, daemonStateView } from './runtimeStatus';

/*
 * 运行状态：桌面端、网关各面与远控连接的真实探测结论，外加配置目录。
 *
 * 全部经网关 HTTP 端点读取，探测失败如实显示未监听，绝不伪造成在线。
 * 这是只读诊断，因此归在「系统设置」而不是「关于」：关于讲产品是什么，
 * 这里讲它此刻跑成什么样。
 *
 * 本组件只输出行，由 SystemSettings 合成一个面板；通知、错误与重启回执
 * 交给外层统一呈现，面板内不再套第二层卡片。
 */

/** 状态行：状态点加语义色文字，不使用灰色药丸。 */
const StatusLabel: React.FC<{ online: boolean; text: string }> = ({ online, text }) => (
  <span className="inline-flex items-center gap-1.5 shrink-0">
    <StateDot state={online ? 'done' : 'idle'} />
    <span className={`text-row ${online ? 'text-ok-ink' : 'text-text-tertiary'}`}>{text}</span>
  </span>
);

interface RuntimeStatusPanelProps {
  /** 重启或探测失败的结论。 */
  onError: (message: string) => void;
  /** 重启成功的回执。 */
  onNotice: (message: string) => void;
}

export const RuntimeStatusPanel: React.FC<RuntimeStatusPanelProps> = ({ onError, onNotice }) => {
  const [info, setInfo] = useState<SystemInfo | null>(null);
  const [restarting, setRestarting] = useState(false);

  const load = useCallback(async () => {
    try {
      setInfo(await ipc.systemInfo());
    } catch (failure) {
      onError(failure instanceof Error ? failure.message : '读取运行状态失败');
    }
  }, [onError]);

  useEffect(() => {
    void load();
  }, [load]);

  /** 重启 Daemon：命令失败不谎报成功，界面结论以重启后的探测为准。 */
  const restartDaemon = useCallback(async () => {
    setRestarting(true);
    try {
      const result = await ipc.gatewayStartService('restart');
      if (result.success) {
        onNotice(result.message);
      } else {
        const output = result.output.trim();
        onError(output.length > 0 ? `${result.message}：${output}` : result.message);
      }
    } catch (failure) {
      onError(failure instanceof Error ? failure.message : '重启 Daemon 服务失败');
    } finally {
      setRestarting(false);
      await load();
    }
  }, [load, onError, onNotice]);

  const daemon = daemonStateView(info?.daemon_state);
  const health = daemonHealthView(info?.daemon_health, info?.daemon_health_error ?? '');
  const daemonTone = daemon.attention
    ? 'text-danger'
    : daemon.dot === 'done'
      ? 'text-ok-ink'
      : 'text-text-secondary';

  return (
    <>
      <ListRow className="py-3">
        <div className="flex-1 min-w-0">
          <Text strong>Daemon 运行状态</Text>
          <Text tone="secondary" size="xs" className="block mt-0.5">
            {info?.daemon_detail ?? '正在查询'}
          </Text>
          <Text tone={health.warning ? 'danger' : 'secondary'} size="xs" className="block mt-0.5">
            {health.text}
          </Text>
        </div>
        <div className="shrink-0 flex items-center gap-2">
          <StateDot state={daemon.dot} />
          <span className={`text-row ${daemonTone}`}>{daemon.label}</span>
          <IconButton
            label={restarting ? '正在重启 Daemon 服务' : '重启 Daemon 服务'}
            disabled={restarting}
            onClick={() => void restartDaemon()}
          >
            {restarting ? (
              <SpinnerIcon size={14} className="animate-spin" />
            ) : (
              <RetryIcon size={14} />
            )}
          </IconButton>
        </div>
      </ListRow>

      {(info?.services ?? []).map((service) => (
        <ListRow key={service.name} className="py-3">
          <div className="flex-1 min-w-0">
            <Text strong>{service.name}</Text>
            <div className="flex items-center gap-1.5 min-w-0 mt-0.5">
              <Text tone="secondary" size="xs" mono truncate className="min-w-0">
                {service.target}
              </Text>
              <Text tone="secondary" size="xs" truncate className="min-w-0">
                {service.detail}
              </Text>
            </div>
          </div>
          <StatusLabel online={service.listening} text={service.listening ? '已监听' : '未监听'} />
        </ListRow>
      ))}

      <ListRow className="py-3">
        <div className="flex-1 min-w-0">
          <Text strong>飞书远控</Text>
          <Text tone="secondary" size="xs" className="block mt-0.5">
            {info?.remote_feishu?.detail !== undefined && info.remote_feishu.detail.length > 0
              ? info.remote_feishu.detail
              : info?.remote_feishu === undefined
                ? '当前没有运行态状态文件，远控未建立连接'
                : '运行态状态文件尚未写入补充信息'}
          </Text>
        </div>
        <StatusLabel
          online={info?.remote_feishu?.connected ?? false}
          text={
            info?.remote_feishu?.connected === true
              ? (info.remote_feishu.state_label ?? '已连接')
              : (info?.remote_feishu?.state_label ?? '未连接')
          }
        />
      </ListRow>

      <ListRow className="py-3">
        <div className="flex-1 min-w-0">
          <Text strong>配置目录</Text>
          <Text tone="secondary" size="xs" className="block mt-0.5">
            人设、记忆、会话与密钥都落在这里
          </Text>
        </div>
        <Text
          mono
          size="xs"
          truncate
          className="shrink-0 max-w-[60%]"
          title={info?.runtime_root ?? ''}
        >
          {info?.runtime_root ?? '未知'}
        </Text>
      </ListRow>
    </>
  );
};
