import React from 'react';
import { ChevronDownIcon } from '../../icons';
import { modelNameOnly } from '../../conversation/format';
import { REASONING_EFFORTS } from '../../conversation/reasoningEffort';
import { Dropdown, MenuItem, MenuLabel } from '../ui/overlays';

/*
 * 输入坞右下角的模型与推理等级切换器。
 *
 * 两项都是可写的真实配置项：主 Agent 每一轮都会重新读取配置，因此这里的改动
 * 对下一轮立即生效，与 TUI 读的是同一份来源。
 *
 * **菜单只有一份实现**：展开、定位、窗口夹取、Esc、方向键与 Home / End、
 * 收起后把焦点还给触发区，全部由共用浮层 `Dropdown` 承担。此前这里自建了第三套
 * 菜单（自己算锚点、自己挂遮罩、只支持鼠标），键盘用户打不开、也走不出去。
 *
 * 两个清单并排成两列：左边模型、右边推理等级。此前是叠在一列里的两段，档位被
 * 十来个模型项压在最下面，每次改思考强度都得先把菜单滚到底；并排之后两列都完整
 * 可见，展开即选，只剩模型列自己承担滚动。
 *
 * 两列是并列而非联动：推理等级是全局配置项，没有按模型区分的档位表，左列选谁
 * 都改变不了右列的候选，做成联动只是假象。
 */

/** 首字母大写的展示形态；取值本身仍是配置里的原始枚举。 */
function displayEffort(value: string): string {
  return value.length === 0 ? value : value[0].toUpperCase() + value.slice(1);
}

export interface ComposerPickerMenuProps {
  /** 当前主模型。 */
  model: string;
  /** 可选模型清单。 */
  models: string[];
  /** 当前推理等级。 */
  effort: string;
  onModelChange: (value: string) => void;
  onReasoningEffortChange: (value: string) => void;
}

/**
 * 菜单本体：模型与推理等级并排两列，各自独立选择。
 *
 * 单独成组件并导出，是为了让静态渲染能覆盖布局本身：菜单只在 `Dropdown` 展开时
 * 渲染，整块组件的静态输出只有触发区，「档位有没有被压进滚动区」这件事就测不到。
 */
export const ComposerPickerMenu: React.FC<ComposerPickerMenuProps> = ({
  model,
  models,
  effort,
  onModelChange,
  onReasoningEffortChange,
}) => (
  <div className="flex items-stretch gap-1">
    {/* 左列：模型。名称长短不一，列宽定死，菜单宽度才不会随当前模型跳动。 */}
    <div data-column="model" className="flex flex-col w-[220px] min-w-0">
      <MenuLabel>模型</MenuLabel>
      {models.length === 0 ? (
        <span className="block px-2.5 py-1.5 text-meta text-text-tertiary">
          配置里还没有可用模型
        </span>
      ) : (
        /* 滚动只发生在这一列：模型清单会随配置变长，档位固定七项。 */
        <div className="max-h-[224px] overflow-y-auto scroll-visible">
          {models.map((option) => (
            <MenuItem
              key={option}
              selected={option === model}
              onSelect={() => onModelChange(option)}
            >
              {option}
            </MenuItem>
          ))}
        </div>
      )}
    </div>

    <div className="w-hairline self-stretch bg-border-hairline" role="separator" />

    {/* 右列：推理等级。七项定高，整列不滚，展开就是全貌。 */}
    <div data-column="effort" className="flex flex-col w-[124px]">
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
    </div>
  </div>
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
}) => (
  <Dropdown
    align="end"
    placement="above"
    closeOnSelect
    /*
     * 整块卡片不再自己滚：滚动交给两列各自承担，卡片高度只由最高的一列决定，
     * 向上展开时不会被窗口下缘夹取。
     */
    menuClassName="max-w-[min(420px,calc(100vw-24px))]"
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
    <ComposerPickerMenu
      model={model}
      models={models}
      effort={effort}
      onModelChange={onModelChange}
      onReasoningEffortChange={onReasoningEffortChange}
    />
  </Dropdown>
);
