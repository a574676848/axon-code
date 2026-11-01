import React, { useCallback, useEffect, useRef, useState } from 'react';
import type { ProfileProbeView } from '../connectionTypes';
import { probeProfiles, switchProfile, useConnectionFact } from '../connectionState';
import { ChevronDownIcon } from '../icons';
import { ROW_ICON_COLUMN, ROW_STATE } from './SidebarRows';
import { Dropdown, MenuItem, MenuLabel, MenuSeparator } from './ui/overlays';
import { StateDot, Text } from './ui/surfaces';

/*
 * 侧栏连接胶囊（设计系统 §14.1.2）。
 *
 * 它是当前生效档案的唯一常驻入口，不是「有事才出现」的提示：尚未连接、甚至快照
 * 还没有取回来时同样显示，因为换个后端只有这一个入口。
 *
 * 读数只取 useConnectionFact()：胶囊自己不发 ipc.connectionState()，也不起轮询；
 * 健康档位直接用快照里的 health.dot，不在本地二次推导，因此胶囊与设置 › 连接
 * 永远说同一句话。
 *
 * 键盘口径按开工合同 §5.2 降级后的那一档：Tab 可达、Enter 触发、Esc 关闭。
 * Dropdown 本期没有方向键导航（菜单宿主只监听 Escape），这里也不额外实现方向键，
 * 免得与共用件各说一套。
 */

/** 尚未取到快照时的占位读数：常驻入口不能因为一次读取还没回来就消失。 */
const PLACEHOLDER_LABEL = '未连接';
/** 档案还没有探测结论时的次要文字。 */
const UNPROBED_LABEL = '未探测';
/** 切档进入重启流程时的过渡文案。 */
const SWITCHING_LABEL = '正在重启以切换后端';

/** 取某条档案的探测结论；没有结论的档案如实写「未探测」。 */
function probeLabel(probes: readonly ProfileProbeView[], id: string): string {
  return probes.find((probe) => probe.id === id)?.message ?? UNPROBED_LABEL;
}

export const ConnectionCapsule: React.FC<{ onManage: () => void }> = ({ onManage }) => {
  const { snapshot, probes } = useConnectionFact();
  /** 切档回执：只在本行就地说明，不弹窗、不改主区域。 */
  const [notice, setNotice] = useState<string | null>(null);
  /**
   * 菜单开合由 Dropdown 自己持有，它不对外提供关闭入口；换 key 重挂载是唯一能
   * 让菜单收起的方式，用于「选定一条档案即收起菜单」。
   */
  const [menuEpoch, setMenuEpoch] = useState(0);
  const triggerRef = useRef<HTMLButtonElement>(null);

  const label = snapshot?.activeLabel ?? PLACEHOLDER_LABEL;
  /** 本机档案的名字与模式同为「本机」，重复一遍只是噪音，因此相等时只留名字。 */
  const mode = snapshot !== null && snapshot.mode !== label ? snapshot.mode : null;
  const healthLabel = snapshot?.health.label ?? PLACEHOLDER_LABEL;
  /*
   * 原生提示与无障碍名称：档案名加一句健康结论。两者同值时只说一遍，免得读屏器
   * 念出「未连接，未连接」；文字被截断时，完整档案名也由它兜住。
   */
  const announcement = healthLabel === label ? label : `${label}，${healthLabel}`;
  const roster = snapshot?.profiles ?? [];

  const handleSwitch = useCallback((id: string) => {
    /*
     * 选定即收起菜单：胶囊贴着窗口底缘，Dropdown 只做下缘夹取、不做上翻，菜单
     * 会盖住胶囊本身与它上方的回执；菜单选完即收也符合清单的常规手感。
     */
    setMenuEpoch((value) => value + 1);
    void switchProfile(id)
      .then((outcome) => {
        setNotice(outcome.started ? SWITCHING_LABEL : outcome.message);
      })
      .catch((failure: unknown) => {
        setNotice(failure instanceof Error ? failure.message : '切换档案失败');
      });
  }, []);

  useEffect(() => {
    // 重挂载让触发按钮成为新节点，焦点否则会掉到 body 上；菜单收起后交回胶囊。
    if (menuEpoch > 0) triggerRef.current?.focus();
  }, [menuEpoch]);

  const handleOpenChange = useCallback((open: boolean) => {
    // 展开时探测一遍：结论只喂给菜单里的次要文字，胶囊自身仍只读快照。
    // 回调必须是稳定引用：Dropdown 每次渲染都会回调一次展开状态，换引用会
    // 让「一开只探一次」变成「每次渲染都探一次」，进而自激成无限探测。
    if (open) void probeProfiles();
  }, []);

  return (
    <>
      {/*
        回执摆在胶囊上方：胶囊贴着窗口底缘，菜单挂在下缘并做下缘夹取（整体上移到
        离窗口底 12px），摆在胶囊下方的回执会被菜单整块盖住，而「无法自动重启，
        请手动重启应用」正是必须被看见的那一档。
      */}
      {notice !== null && (
        <p role="status" className="px-3 text-meta leading-5 text-text-secondary">
          {notice}
        </p>
      )}

      <Dropdown
        key={menuEpoch}
        className="w-full"
        onOpenChange={handleOpenChange}
        trigger={({ open, toggle }) => (
          <button
            ref={triggerRef}
            type="button"
            onClick={toggle}
            aria-haspopup="menu"
            aria-expanded={open}
            aria-label={`连接：${announcement}`}
            title={announcement}
            className={`${ROW_STATE} w-full h-8 px-3 bg-surface hairline border-border-input text-left select-none cursor-pointer hover:bg-hover hover:border-border-strong active:bg-active focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus`}
          >
            {/* 健康点参与侧栏栅格：24px 图标列承载状态点，8px 间隔后接文字列。 */}
            <span className={ROW_ICON_COLUMN}>
              <StateDot state={snapshot?.health.dot ?? 'idle'} />
            </span>
            <Text size="row" truncate className="flex-1 min-w-0">
              {label}
            </Text>
            {mode !== null && (
              <Text size="micro" tone="tertiary" className="shrink-0">
                · {mode}
              </Text>
            )}
            <ChevronDownIcon
              size={12}
              className={`shrink-0 text-text-tertiary transition-transform duration-150 ${
                open ? 'rotate-180' : ''
              }`}
            />
          </button>
        )}
      >
        {/*
          档案清单：每一项点一下就是一次切档。次要文字给探测结论，没有结论的档案
          如实写「未探测」，不拿当前健康态去冒充别人的结论。
        */}
        {roster.length === 0 ? (
          <MenuLabel>{snapshot === null ? '正在读取连接档案' : '暂无连接档案'}</MenuLabel>
        ) : (
          roster.map((profile) => (
            <MenuItem
              key={profile.id}
              selected={profile.active}
              hint={probeLabel(probes, profile.id)}
              onSelect={() => handleSwitch(profile.id)}
            >
              {profile.label}
            </MenuItem>
          ))
        )}
        <MenuSeparator />
        <MenuItem onSelect={onManage}>管理连接…</MenuItem>
      </Dropdown>
    </>
  );
};
