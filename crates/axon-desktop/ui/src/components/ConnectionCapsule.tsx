import React, { useCallback, useEffect, useRef, useState } from 'react';
import { connectionSummary, readoutText } from '../connectionReadout';
import { latencyText, switchProfile, useConnectionFact } from '../connectionState';
import type { McpHeaderReading } from '../conversation/mcpReading';
import { sidebarReadouts } from '../conversation/sidebarReading';
import { ChevronDownIcon, PulseIcon } from '../icons';
import { ROW_ICON_COLUMN, ROW_STATE } from './SidebarRows';
import { Dropdown, MenuItem, MenuLabel, MenuSeparator } from './ui/overlays';
import { STATE_ICON_INK, Text } from './ui/surfaces';

/*
 * 侧栏连接胶囊（设计系统 §14.1.2）。
 *
 * 形态与顶部导航、设置行同一套侧栏行：24px 图标列放线性连接图形，延迟读数收在行尾，
 * 不再自带描边与卡片底。底栏只有它与设置行两行，两行长得不一样会被读成两种控件。
 *
 * 它是当前生效档案的唯一常驻入口，不是「有事才出现」的提示：尚未连接、甚至快照
 * 还没有取回来时同样显示，因为换个后端只有这一个入口。
 *
 * 行内自左向右读作「档案 MCP 技能 延迟」：档案名之后是这台机器此刻挂着什么
 * （MCP 台数、技能条数），行尾是这一档延迟多少。前两项因此不承担任何点击语义，
 * 整行仍只有一个动作：展开档案清单。
 *
 * 健康档位不另画状态点，由行首连接图形自己的颜色承担：已连接绿、连不上红、
 * 需要人过目琥珀、尚未探测灰，连接中则让同一枚图形转起来。颜色不单独承载状态，
 * 完整结论仍在整行的无障碍名称与原生提示里。行尾因此只剩延迟读数与展开箭头。
 *
 * 读数只取 useConnectionFact()：胶囊自己不发 ipc.connectionState()，也不起轮询；
 * 健康档位直接用快照里的 health.dot，不在本地二次推导，因此胶囊与设置 › 连接
 * 永远说同一句话。行尾只在健康档写延迟；其余档位（含「后台已停机」这种网关通、
 * 后台租约失效的降级态）改显健康结论，两者不会同时出现，也不会写成 0。
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

/**
 * 条目分隔圆点。
 *
 * 这一行有四个读数，用空白分隔会被读成一串连字（`MCP 3 技能 32 7 毫秒`）；圆点是排版
 * 记号而不是内容，因此取三级文本色、不进无障碍树。
 */
const ReadoutSeparator: React.FC = () => (
  <span aria-hidden="true" className="shrink-0 text-micro text-text-tertiary">
    ·
  </span>
);

export interface ConnectionCapsuleProps {
  /** 打开设置 › 连接。 */
  onManage: () => void;
  /** MCP 运行态读数；为空时这一段不占位。 */
  mcpReading?: McpHeaderReading | null;
  /** 已安装技能条数；清单未就绪或零条时不占位。 */
  skillCount?: number | null;
}

export const ConnectionCapsule: React.FC<ConnectionCapsuleProps> = ({
  onManage,
  mcpReading = null,
  skillCount = null,
}) => {
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
  /** 健康档位：只取快照结论，不在本地二次推导。 */
  const healthState = snapshot?.health.dot ?? 'idle';
  /*
   * 行首图形同时承载「这是一条连接」与「此刻好不好」：图形不变，颜色随档位走。
   * 连接中是唯一没有稳定状态的档位，于是让同一枚图形转起来（全站共用 .state-spinner）。
   */
  const healthInk = STATE_ICON_INK[healthState];
  const healthSpinning = healthState === 'ongoing';
  /** 行尾读数：有延迟就写延迟，没有才写健康结论。 */
  /** 行尾读数：健康时是延迟，其余档位是健康结论（设计系统 §14.1.2）。 */
  const latency = snapshot === null ? null : latencyText(snapshot.latencyMs);
  const readout = readoutText(snapshot?.health ?? null, latency) ?? healthLabel;
  /** 档案名之后的辅助读数：MCP 台数与技能条数，缺数据即整段不占位。 */
  const auxReadouts = sidebarReadouts(mcpReading, skillCount);
  /*
   * 原生提示与无障碍名称：档案名加健康结论，健康时再补一句延迟、降级时补一句可执行说明。
   * 两者同值时只说一遍，免得读屏器念出「未连接，未连接」；文字被截断时，完整档案名也由它兜住。
   */
  const summary = connectionSummary(label, snapshot?.health ?? null, latency);
  /*
   * 整行只有一个无障碍名称：按钮的 aria-label 会盖住子节点文字，因此辅助读数
   * 也一并写进来，读屏器不必回头去猜这一行还有几台 MCP、几条技能。
   */
  const announcement =
    auxReadouts.length === 0
      ? summary
      : `${summary}；${auxReadouts.map((item) => item.title).join('；')}`;
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
        <p role="status" className="text-meta leading-5 text-text-secondary">
          {notice}
        </p>
      )}

      <Dropdown
        key={menuEpoch}
        className="w-full"
        // 清单靠右缘对齐并向上展开：这一行的可点提示（读数与展开箭头）都在行尾，
        // 菜单贴左缘会落在图标下方，向下展开又会压在窗口底缘并盖住被点的这一行。
        align="end"
        placement="above"
        trigger={({ open, toggle }) => (
          <button
            ref={triggerRef}
            type="button"
            onClick={toggle}
            aria-haspopup="menu"
            aria-expanded={open}
            aria-label={`连接：${announcement}`}
            title={announcement}
            className={`${ROW_STATE} w-full h-8 text-left select-none cursor-pointer hover:bg-hover active:bg-active focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus`}
          >
            {/*
              图标列承载「连接」这一语义本身：图形说明这一行是什么，颜色说明它此刻好不好。
              两者不拆成两枚元素，因此底栏这一行只有一枚状态标记，且它在行首。
            */}
            <span
              className={`${ROW_ICON_COLUMN} ${healthInk} ${healthSpinning ? 'state-spinner' : ''}`}
            >
              <PulseIcon size={14} />
            </span>
            {/*
              一行读作一句话：档案名 · MCP 台数 · 技能条数 · 延迟读数，条目之间用同一个圆点
              分隔，末尾留给展开箭头。读数只给原生提示，不加第二层按钮：整行只有一个动作。
            */}
            <span className="flex-1 min-w-0 flex items-center gap-1.5">
              <Text size="body" truncate className="min-w-0 leading-5">
                {label}
              </Text>
              {auxReadouts.map((item) => (
                <React.Fragment key={item.kind}>
                  <ReadoutSeparator />
                  <Text
                    size="micro"
                    tone={item.tone === 'critical' ? 'danger' : 'tertiary'}
                    title={item.title}
                    className="shrink-0 tabular-nums"
                  >
                    {item.text}
                  </Text>
                </React.Fragment>
              ))}
              <ReadoutSeparator />
              <Text size="micro" tone="tertiary" className="shrink-0 tabular-nums">
                {readout}
              </Text>
            </span>
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
