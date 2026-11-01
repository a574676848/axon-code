import React, { useEffect, useState } from 'react';
import * as ipc from '../../ipc';
import type { RemoteStatus } from '../../types';
import { Chip, Text } from '../ui/surfaces';

/*
 * 远控分区底部的飞书只读一行。
 *
 * 这行只读：飞书状态来自适配器自己的状态文件，不由配置表单决定，因此放在表单
 * 之外、分区底部，一句话说清「连没连上」。读不到时如实说读不到，不显示成「未配置」。
 */
export const RemoteFeishuRow: React.FC = () => {
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let active = true;
    ipc
      .systemInfo()
      .then((info) => {
        if (!active) return;
        setStatus(info.remote_feishu ?? null);
        setLoaded(true);
      })
      .catch(() => {
        if (!active) return;
        setFailed(true);
        setLoaded(true);
      });
    return () => {
      active = false;
    };
  }, []);

  const tone = failed ? 'danger' : status === null ? 'neutral' : status.connected ? 'ok' : 'warning';
  const label = failed
    ? '飞书远控状态读取失败'
    : !loaded
      ? '正在读取飞书远控状态'
      : status === null
        ? '飞书远控未配置'
        : status.state_label;

  return (
    <div className="mt-3 pt-3 hairline-t border-border-hairline flex flex-wrap items-center gap-2">
      <Text tone="secondary" size="xs" className="shrink-0">
        飞书远控
      </Text>
      <Chip tone={tone}>{label}</Chip>
      {status !== null && status.detail.length > 0 && (
        <Text tone="tertiary" size="xs" className="min-w-0 truncate">
          {status.detail}
        </Text>
      )}
    </div>
  );
};
