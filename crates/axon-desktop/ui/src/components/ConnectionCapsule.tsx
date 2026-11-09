import React, { useCallback, useEffect, useRef, useState } from 'react';
import { latencyText, switchProfile, useConnectionFact } from '../connectionState';
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
 * 永远说同一句话。右端固定给延迟读数，取不到延迟时才退回健康结论（未连接、连不上），
 * 两者不会同时出现，也不会写成 0。
 *
 * 一级（胶囊本身）只负责展开清单，不承担切档：切档要重启应用，误触代价太大。
 * 二级清单只在真有多份档案时才给出可切换项，且不复述任何连接状态：那些结论属于
 * 生效档案本身，挂在别的档案名后面等于替它下结论。
 *
 * 键盘口径按开工合同 §5.2 降级后的那一档：Tab 可达、Enter 触发、Esc 关闭。
 * Dropdown 本期没有方向键导航（菜单宿主只监听 Escape），这里也不额外实现方向键，
 * 免得与共用件各说一套。
 */

/** 尚未取到快照时的占位读数：常驻入口不能因为一次读取还没回来就消失。 */
const PLACEHOLDER_LABEL = '未连接';
/** 切档进入重启流程时的过渡文案。 */
const SWITCHING_LABEL = '正在重启以切换后端';

export const ConnectionCapsule: React.FC<{ onManage: () => void }> = ({ onManage }) => {
  const { snapshot } = useConnectionFact();
  /** 切档回执：只在本行就地说明，不弹窗、不改主区域。 */
  const [notice, setNotice] = useState<string | null>(null);
  /**
   * 菜单开合由 Dropdown 自己持有，它不对外提供关闭入口；换 key 重挂载是唯一能
   * 让菜单收起的方式，用于「选定一条档案即收起菜单」。
   */
  const [menuEpoch, setMenuEpoch] = useState(0);
  const triggerRef = useRef<HTMLButtonElement>(null);

  const label = snapshot?.activeLabel ?? PLACEHOLDER_LABEL;
  const healthLabel = snapshot?.health.label ?? PLACEHOLDER_LABEL;
  /** 右端读数：有延迟就写延迟，没有才写健康结论。 */
  const latency = snapshot === null ? null : latencyText(snapshot.latencyMs);
  const readout = latency ?? healthLabel;
  /*
   * 原生提示与无障碍名称：档案名加健康结论，有延迟时再补一句。两者同值时只说一遍，
   * 免得读屏器念出「未连接，未连接」；文字被截断时，完整档案名也由它兜住。
   */
  const summary = healthLabel === label ? label : `${label}，${healthLabel}`;
  const announcement = latency === null ? summary : `${summary}，延迟 ${latency}`;
  /** 可切换的档案：当前生效那一份不在其中，避免一次「选了自己」的重启。 */
  const targets = (snapshot?.profiles ?? []).filter((profile) => !profile.active);
  const rosterEmpty = snapshot !== null && (snapshot.profiles ?? []).length === 0;

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
            <Text size="micro" tone="tertiary" className="shrink-0 tabular-nums">
              {readout}
            </Text>
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
          二级清单：多份档案时才列出可切换项，且只写名字，不复述连接状态。
          只有一份档案（或一份都还没有）时不列条目，菜单只留「管理连接…」。
        */}
        {rosterEmpty ? (
          <MenuLabel>暂无连接档案</MenuLabel>
        ) : (
          targets.map((profile) => (
            <MenuItem key={profile.id} onSelect={() => handleSwitch(profile.id)}>
              {profile.label}
            </MenuItem>
          ))
        )}
        {targets.length > 0 && <MenuSeparator />}
        <MenuItem onSelect={onManage}>管理连接…</MenuItem>
      </Dropdown>
    </>
  );
};
