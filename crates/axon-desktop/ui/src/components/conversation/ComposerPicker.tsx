import React from 'react';
import { ChevronDownIcon } from '../../icons';
import { modelNameOnly } from '../../conversation/format';
import { REASONING_EFFORTS } from '../../conversation/reasoningEffort';
import { Dropdown, MenuItem, MenuLabel, MenuSeparator } from '../ui/overlays';

/*
 * 输入坞右下角的模型与推理等级切换器。
 *
 * 两项都是可写的真实配置项：主 Agent 每一轮都会重新读取配置，因此这里的改动
 * 对下一轮立即生效，与 TUI 读的是同一份来源。
 *
 * **菜单只有一份实现**：展开、定位、窗口夹取、Esc、方向键与 Home / End、
 * 收起后把焦点还给触发区，全部由共用浮层 `Dropdown` 承担。此前这里自建了第三套
 * 菜单（自己算锚点、自己挂遮罩、只支持鼠标），键盘用户打不开、也走不出去。
 * 现在两级改成一列两段：模型在上、推理等级在下，方向键可以在整份清单里连续移动。
 */

/** 首字母大写的展示形态；取值本身仍是配置里的原始枚举。 */
function displayEffort(value: string): string {
  return value.length === 0 ? value : value[0].toUpperCase() + value.slice(1);
}

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
}) => (
  <Dropdown
    align="end"
    placement="above"
    closeOnSelect
    menuClassName="min-w-[220px] max-w-[min(320px,calc(100vw-24px))] max-h-[320px] overflow-y-auto scroll-visible"
    trigger={({ open, toggle }) => (
      <button
        type="button"
        onClick={toggle}
        disabled={configBusy || configLocked}
        title={configLocked ? '自动化会话的模型和思考强度已固定' : undefined}
        aria-haspopup="menu"
        aria-expanded={open}
        /* 无障碍名称带上当前取值：按钮上的字被读屏器念成名称时，这一句才是完整读数。 */
        aria-label={`模型与推理等级：${modelNameOnly(model) || '未选择模型'}，${displayEffort(effort)}`}
        className="h-7 pl-2.5 pr-1.5 inline-flex items-center gap-1 rounded-full text-row text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus disabled:cursor-wait disabled:opacity-40"
      >
        <span className="max-w-[220px] truncate">{modelNameOnly(model) || '未选择模型'}</span>
        <span className="text-text-tertiary">{displayEffort(effort)}</span>
        <ChevronDownIcon
          size={12}
          className={`text-text-tertiary transition-transform duration-150 ${open ? 'rotate-180' : ''}`}
        />
      </button>
    )}
  >
    <MenuLabel>模型</MenuLabel>
    {models.length === 0 ? (
      <span className="block px-2.5 py-1.5 text-meta text-text-tertiary">配置里还没有可用模型</span>
    ) : (
      models.map((option) => (
        <MenuItem
          key={option}
          selected={option === model}
          onSelect={() => onModelChange(option)}
        >
          {option}
        </MenuItem>
      ))
    )}
    <MenuSeparator />
    <MenuLabel>推理等级</MenuLabel>
    {REASONING_EFFORTS.map((option) => (
      <MenuItem
        key={option}
        selected={option === effort}
        onSelect={() => onReasoningEffortChange(option)}
      >
        {displayEffort(option)}
      </MenuItem>
    ))}
  </Dropdown>
);
