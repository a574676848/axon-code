import React, { useCallback, useEffect, useState } from 'react';
import { Switch } from '../ui/controls';
import { Banner, Panel, Text, ToggleField } from '../ui/surfaces';
import * as ipc from '../../ipc';
import { AlertCircleIcon, CheckIcon } from '../../icons';
import { RuntimeStatusPanel } from './RuntimeStatusPanel';

/*
 * 系统设置：桌面外壳自身的偏好与运行状态。
 * 前者是「改了立刻生效、且与运行态无关」的开关，后者是只读探测结论；
 * 需要重启或与后端协商的项不放这，产品身份与发布渠道属于「关于」。
 *
 * 通知与运行状态读作同一个对象的两个部分，因此合成一个面板：
 * 一行通知开关，其余是发丝线分隔的只读状态行，不再堆五张独立卡片。
 */

export const SystemSettings: React.FC = () => {
  const [notifications, setNotifications] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void ipc
      .desktopPreferences()
      .then((value) => {
        if (!cancelled) setNotifications(value.notificationsEnabled);
      })
      .catch((failure: unknown) => {
        if (cancelled) return;
        setError(failure instanceof Error ? failure.message : '读取系统设置失败');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const handleError = useCallback((message: string) => {
    setError(message);
    setNotice(null);
  }, []);

  const handleNotice = useCallback((message: string) => {
    setNotice(message);
    setError(null);
  }, []);

  const toggleNotifications = useCallback(async (next: boolean) => {
    setNotifications(next);
    setError(null);
    try {
      const saved = await ipc.desktopSetNotifications(next);
      setNotifications(saved.notificationsEnabled);
    } catch (failure: unknown) {
      setError(failure instanceof Error ? failure.message : '保存系统设置失败');
      // 写入失败就回到磁盘上的真实值，不能让开关停在未生效的状态上。
      try {
        const current = await ipc.desktopPreferences();
        setNotifications(current.notificationsEnabled);
      } catch {
        setNotifications(null);
      }
    }
  }, []);

  return (
    <div className="space-y-3">
      {notice !== null && <Banner tone="success" icon={<CheckIcon size={13} />} title={notice} />}
      {error !== null && (
        <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={`系统设置不可用：${error}`} />
      )}

      <Panel clip>
        <div className="px-3.5 py-3 hairline-b border-border-hairline">
          <ToggleField
            label="消息通知"
            description="轮次结束时发送系统通知，标题区分完成、失败与已中断；关闭后不再发送"
            control={
              notifications === null ? (
                <Text tone="tertiary" size="xs">
                  读取中
                </Text>
              ) : (
                <Switch
                  checked={notifications}
                  onChange={(next) => void toggleNotifications(next)}
                  label="消息通知"
                />
              )
            }
          />
        </div>

        <RuntimeStatusPanel onError={handleError} onNotice={handleNotice} />
      </Panel>
    </div>
  );
};
