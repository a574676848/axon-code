import React from 'react';
import { createPortal } from 'react-dom';
import { ChevronDownIcon, ChevronRightIcon } from '../../icons';
import { modelNameOnly } from '../../conversation/format';
import { Z_MENU, Z_MENU_SCRIM } from './chatChrome';
import { MENU_ANCHOR_GAP, MENU_PADDING, MenuRow } from './MenuPanel';

/*
 * 输入坞右下角的模型与推理等级切换器。
 *
 * 两项都是可写的真实配置项：主 Agent 每一轮都会重新读取配置，因此这里的改动
 * 对下一轮立即生效，与 TUI 读的是同一份来源。抽成独立组件只是为了控制输入坞的行数，
 * 行为与它内联在这里时完全一致。
 *
 * 菜单走 portal 挂到 body：输入坞外壳是圆角卡片，菜单留在里面会被卡片边界裁掉，
 * 也会被同层背景压住。卡片几何与行高沿用 MenuPanel（8px 圆角、4px 内边距、32px 行）。
 */

/** 浮层与视口边缘的最小留白。 */
const VIEWPORT_MARGIN = 8;

/** 推理等级候选值，与后端配置枚举逐一对应。 */
const REASONING_EFFORTS = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const;

/** 首字母大写的展示形态；取值本身仍是配置里的原始枚举。 */
function displayEffort(value: string): string {
  return value.length === 0 ? value : value[0].toUpperCase() + value.slice(1);
}

type PickerView = 'root' | 'model' | 'effort';

const PickerRow: React.FC<{ label: string; value: string; onClick: () => void }> = ({
  label,
  value,
  onClick,
}) => (
  <MenuRow
    role="menuitem"
    label={label}
    value={value}
    trailing={<ChevronRightIcon size={12} className="shrink-0 text-text-tertiary" />}
    onClick={onClick}
  />
);

const PickerOption: React.FC<{ label: string; selected: boolean; onClick: () => void }> = ({
  label,
  selected,
  onClick,
}) => (
  <MenuRow role="option" ariaSelected={selected} selected={selected} label={label} onClick={onClick} />
);

export interface ComposerPickerProps {
  /** 当前主模型。 */
  model: string;
  /** 可选模型清单。 */
  models: string[];
  /** 当前推理等级。 */
  effort: string;
  /** 配置写入期间暂停切换。 */
  configBusy: boolean;
  /** 自动化会话的模型与思考强度由后端锁定，界面不给改。 */
  configLocked: boolean;
  onModelChange: (value: string) => void;
  onReasoningEffortChange: (value: string) => void;
}

export const ComposerPicker: React.FC<ComposerPickerProps> = ({
  model,
  models,
  effort,
  configBusy,
  configLocked,
  onModelChange,
  onReasoningEffortChange,
}) => {
  const [view, setView] = React.useState<PickerView | null>(null);
  /** 菜单锚点：触发按钮右缘与其上沿；null 表示菜单未打开。 */
  const [anchor, setAnchor] = React.useState<{ right: number; bottom: number } | null>(null);
  const triggerRef = React.useRef<HTMLButtonElement>(null);

  const toggle = () => {
    if (view !== null) {
      setView(null);
      setAnchor(null);
      return;
    }
    const rect = triggerRef.current?.getBoundingClientRect();
    if (rect === undefined) return;
    setAnchor({
      right: Math.max(VIEWPORT_MARGIN, window.innerWidth - rect.right),
      bottom: window.innerHeight - rect.top + MENU_ANCHOR_GAP,
    });
    setView('root');
  };

  // 窗口尺寸变化时收起：菜单是 fixed 定位，留在原地会与按钮脱节。
  React.useEffect(() => {
    if (view === null) return undefined;
    const close = () => {
      setView(null);
      setAnchor(null);
    };
    window.addEventListener('resize', close);
    return () => window.removeEventListener('resize', close);
  }, [view]);

  return (
    <div className="relative">
      <button
        ref={triggerRef}
        type="button"
        onClick={toggle}
        disabled={configBusy || configLocked}
        title={configLocked ? '自动化会话的模型和思考强度已固定' : undefined}
        aria-haspopup="menu"
        aria-expanded={view !== null}
        className="h-7 pl-2.5 pr-1.5 inline-flex items-center gap-1 rounded-full text-row text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary disabled:cursor-wait disabled:opacity-40"
      >
        <span className="max-w-[220px] truncate">{modelNameOnly(model) || '未选择模型'}</span>
        <span className="text-text-tertiary">{displayEffort(effort)}</span>
        <ChevronDownIcon size={12} className="text-text-tertiary" />
      </button>

      {view !== null &&
        anchor !== null &&
        createPortal(
          <>
            <div
              className={`fixed inset-0 ${Z_MENU_SCRIM}`}
              role="presentation"
              onClick={() => {
                setView(null);
                setAnchor(null);
              }}
            />
            <div
              role="menu"
              style={{
                right: anchor.right,
                bottom: anchor.bottom,
                width: 'max-content',
                minWidth: view === 'effort' ? 108 : 184,
                maxWidth: 'min(320px, calc(100vw - 16px))',
              }}
              className={`fixed ${Z_MENU} flex flex-col ${MENU_PADDING} rounded-sm bg-surface hairline border-border-hairline shadow-menu overflow-hidden`}
            >
              {view === 'root' ? (
                <>
                  <PickerRow
                    label="模型"
                    value={modelNameOnly(model) || '未配置'}
                    onClick={() => setView('model')}
                  />
                  <PickerRow
                    label="推理等级"
                    value={displayEffort(effort)}
                    onClick={() => setView('effort')}
                  />
                </>
              ) : view === 'model' ? (
                <div className="flex flex-col max-h-[260px] overflow-y-auto scroll-visible">
                  {models.length === 0 ? (
                    <div className="px-2.5 py-1.5 text-meta text-text-tertiary">
                      配置里还没有可用模型
                    </div>
                  ) : (
                    models.map((option) => (
                      <PickerOption
                        key={option}
                        label={option}
                        selected={option === model}
                        onClick={() => {
                          onModelChange(option);
                          setView(null);
                          setAnchor(null);
                        }}
                      />
                    ))
                  )}
                </div>
              ) : (
                <div className="flex flex-col">
                  {REASONING_EFFORTS.map((option) => (
                    <PickerOption
                      key={option}
                      label={displayEffort(option)}
                      selected={option === effort}
                      onClick={() => {
                        onReasoningEffortChange(option);
                        setView(null);
                        setAnchor(null);
                      }}
                    />
                  ))}
                </div>
              )}
            </div>
          </>,
          document.body,
        )}
    </div>
  );
};
