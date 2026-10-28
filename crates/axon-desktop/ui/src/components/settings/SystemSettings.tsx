import React, { useCallback, useEffect, useState } from 'react';
import { Button, Switch } from '../ui/controls';
import { Banner, Panel, Text, ToggleField } from '../ui/surfaces';
import * as ipc from '../../ipc';
import { applyTheme } from '../../theme';
import { isSupported, useGlobalAutoRead, voicePreferences } from '../../speech';
import { VOICE_UI_ENABLED } from '../../speech/enabled';
import { AlertCircleIcon, CheckIcon } from '../../icons';
import type { ThemeId } from '../../types';
import { RuntimeStatusPanel } from './RuntimeStatusPanel';
import { VoiceInputSettings } from './VoiceInputSettings';
import { ShortcutsDialog } from './ShortcutsDialog';

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
  subtitle: string;
  sidebar: string;
  canvas: string;
  card: string;
  accent: string;
  /** 冷杉多一层松雾微光。 */
  wash?: string;
}> = [
  {
    value: 'paper',
    label: '纸白',
    subtitle: '清透纯净亮白',
    sidebar: 'bg-swatch-paper-sidebar',
    canvas: 'bg-swatch-paper-canvas',
    card: 'bg-swatch-paper-card',
    accent: 'bg-swatch-paper-accent',
  },
  {
    value: 'forest',
    label: '冷杉',
    subtitle: '松雾冷矿青白',
    sidebar: 'bg-swatch-forest-sidebar',
    canvas: 'bg-swatch-forest-canvas',
    card: 'bg-swatch-forest-card',
    accent: 'bg-swatch-forest-accent',
    wash: 'bg-swatch-forest-wash',
  },
  {
    value: 'obsidian',
    label: '黑曜石',
    subtitle: '深邃钨钢暗色',
    sidebar: 'bg-swatch-obsidian-sidebar',
    canvas: 'bg-swatch-obsidian-canvas',
    card: 'bg-swatch-obsidian-card',
    accent: 'bg-swatch-obsidian-accent',
  },
];

function themeDisplayName(theme: ThemeId): string {
  if (theme === 'obsidian') return '黑曜石';
  if (theme === 'forest') return '冷杉';
  return '纸白';
}

export const SystemSettings: React.FC = () => {
  const [notifications, setNotifications] = useState<boolean | null>(null);
  /** 快捷键速查弹窗是否打开。 */
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [theme, setTheme] = useState<ThemeId | null>(null);
  const voiceAutoRead = useGlobalAutoRead();
  const voiceSupported = isSupported();
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
                {theme === null ? '读取中' : `当前使用${themeDisplayName(theme)}`}
              </Text>
            </div>
            <div role="radiogroup" aria-label="界面主题" className="flex shrink-0 gap-2">
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
                    className={`w-[124px] rounded-md border p-1.5 text-left transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-50 ${
                      active
                        ? 'border-primary bg-hover/60 shadow-lift'
                        : 'border-border hover:border-border-strong'
                    }`}
                  >
                    {/* 微缩工作台：左侧托盘底座 + 右侧内嵌圆角画布 + 内容卡 + 底部输入坞与主色点 */}
                    <span
                      className={`relative flex h-12 overflow-hidden rounded-xs p-1 gap-1 hairline border-border-hairline ${item.sidebar}`}
                    >
                      <span className="w-4 h-full flex flex-col gap-1 pt-0.5 pl-0.5">
                        <span className={`w-2.5 h-1.5 rounded-[1px] ${item.card}`} />
                        <span className="w-2.5 h-1 rounded-[1px] opacity-40 bg-current" />
                      </span>
                      <span
                        className={`relative flex-1 h-full rounded-[3px] overflow-hidden ${item.canvas}`}
                      >
                        {item.wash !== undefined && (
                          <span className={`absolute inset-0 opacity-35 ${item.wash}`} />
                        )}
                        <span
                          className={`absolute left-1.5 right-1.5 top-1.5 h-2 rounded-[1.5px] ${item.card}`}
                        />
                        <span
                          className={`absolute left-1.5 right-1.5 bottom-1.5 h-2.5 rounded-[2px] flex items-center justify-end pr-0.5 ${item.card}`}
                        >
                          <span className={`w-1.5 h-1.5 rounded-full ${item.accent}`} />
                        </span>
                      </span>
                    </span>
                    <span className="mt-1.5 flex items-center px-0.5">
                      <span className="min-w-0 flex-1">
                        <Text size="xs" strong={active} truncate>
                          {item.label}
                        </Text>
                      </span>
                      <span
                        className={`ml-auto shrink-0 grid h-3.5 w-3.5 place-items-center rounded-full ${
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

        {/*
          快捷键速查：只读清单，点开是「有哪些键可按」，不做键位自定义。
          放在系统设置最前：它是外壳自身的操作约定，与下面的通知开关同一层级。
        */}
        <div className="flex items-center justify-between gap-3 px-3.5 py-3 hairline-b border-border-hairline">
          <div className="min-w-0 space-y-0.5">
            <Text size="sm">快捷键</Text>
            <Text tone="secondary" size="xs" className="block">
              查看当前支持的快捷键与输入操作
            </Text>
          </div>
          <Button variant="outline" size="sm" onClick={() => setShortcutsOpen(true)}>
            查看快捷键
          </Button>
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

        {VOICE_UI_ENABLED && <VoiceInputSettings />}

        {VOICE_UI_ENABLED && (
          <div className="px-3.5 py-3 hairline-b border-border-hairline">
            <ToggleField
              label="语音播报"
              description={
                voiceSupported
                  ? '新回复完成后自动朗读，音色取人设页里选中的那一档；每个会话还能在标题栏单独关闭'
                  : '当前窗口不支持音频播放，语音播报不可用'
              }
              control={
                <Switch
                  checked={voiceAutoRead}
                  disabled={!voiceSupported}
                  onChange={(next) => voicePreferences.setGlobalAutoRead(next)}
                  label="语音播报"
                />
              }
            />
          </div>
        )}

        <RuntimeStatusPanel onError={handleError} onNotice={handleNotice} />

      <ShortcutsDialog open={shortcutsOpen} onClose={() => setShortcutsOpen(false)} />
      </Panel>
    </div>
  );
};
