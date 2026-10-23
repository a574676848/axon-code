import React, { useCallback, useEffect, useState } from 'react';
import { Switch } from '../ui/controls';
import { Banner, Panel, Text, ToggleField } from '../ui/surfaces';
import * as ipc from '../../ipc';
import { applyTheme } from '../../theme';
import { AlertCircleIcon, CheckIcon } from '../../icons';
import type { ThemeId } from '../../types';
import { RuntimeStatusPanel } from './RuntimeStatusPanel';

/*
 * 系统设置：桌面外壳自身的偏好与运行状态。
 * 前者是「改了立刻生效、且与运行态无关」的开关，后者是只读探测结论；
 * 需要重启或与后端协商的项不放这，产品身份与发布渠道属于「关于」。
 *
 * 主题用缩略卡而不是分段控件：两套主题的差别主要在底色与留白，
 * 光看名字选不出来，给一块缩小的界面比两个词更省判断。
 */

/** 主题选择卡：缩略图用的色板是固定的，描述的是「那套主题长什么样」。 */
const THEME_CARDS: ReadonlyArray<{
  value: ThemeId;
  label: string;
  canvas: string;
  card: string;
  /** 森林多一层草色薄雾，纸白没有。 */
  wash?: string;
}> = [
  {
    value: 'paper',
    label: '纸白',
    canvas: 'bg-swatch-paper-canvas',
    card: 'bg-swatch-paper-card',
  },
  {
    value: 'forest',
    label: '森林',
    canvas: 'bg-swatch-forest-canvas',
    card: 'bg-swatch-forest-card',
    wash: 'bg-swatch-forest-wash',
  },
];

export const SystemSettings: React.FC = () => {
  const [notifications, setNotifications] = useState<boolean | null>(null);
  const [theme, setTheme] = useState<ThemeId | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void ipc
      .desktopPreferences()
      .then((value) => {
        if (cancelled) return;
        setNotifications(value.notificationsEnabled);
        setTheme(value.theme);
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

  const chooseTheme = useCallback(async (next: ThemeId) => {
    // 先上屏再落盘：主题是纯外观，等待往返只会让切换显得迟钝。
    setTheme(next);
    applyTheme(next);
    setError(null);
    try {
      const saved = await ipc.desktopSetTheme(next);
      setTheme(saved.theme);
      applyTheme(saved.theme);
    } catch (failure: unknown) {
      setError(failure instanceof Error ? failure.message : '保存主题失败');
      // 写入失败回到磁盘上的真实主题，界面与服务端不能各说一套。
      try {
        const current = await ipc.desktopPreferences();
        setTheme(current.theme);
        applyTheme(current.theme);
      } catch {
        setTheme(null);
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
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0 space-y-0.5">
              <Text size="sm">主题</Text>
              <Text tone="secondary" size="xs" className="block">
                {theme === null ? '读取中' : `当前使用${theme === 'forest' ? '森林' : '纸白'}`}
              </Text>
            </div>
            <div role="radiogroup" aria-label="界面主题" className="flex shrink-0 gap-2.5">
              {THEME_CARDS.map((item) => {
                const active = theme === item.value;
                return (
                  <button
                    key={item.value}
                    type="button"
                    role="radio"
                    aria-checked={active}
                    disabled={theme === null}
                    onClick={() => void chooseTheme(item.value)}
                    className={`w-[136px] rounded-md border p-1.5 text-left transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-50 ${
                      active ? 'border-primary' : 'border-border hover:border-border-strong'
                    }`}
                  >
                    {/* 缩略界面：一块底色加两条白卡，够看出「更白」与「更绿」。 */}
                    <span className={`relative block h-11 overflow-hidden rounded-sm ${item.canvas}`}>
                      {item.wash !== undefined && (
                        <span className={`absolute inset-0 opacity-40 ${item.wash}`} />
                      )}
                      <span
                        className={`absolute left-2 right-2 top-2 h-2.5 rounded-[2px] ${item.card}`}
                      />
                      <span
                        className={`absolute left-2 right-6 top-6 h-2.5 rounded-[2px] ${item.card}`}
                      />
                    </span>
                    <span className="mt-1.5 flex items-center px-0.5">
                      <Text size="xs">{item.label}</Text>
                      <span
                        className={`ml-auto grid h-3.5 w-3.5 place-items-center rounded-full ${
                          active ? 'bg-primary text-white' : 'border border-border-strong'
                        }`}
                      >
                        {active && <CheckIcon size={8} />}
                      </span>
                    </span>
                  </button>
                );
              })}
            </div>
          </div>
        </div>

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
