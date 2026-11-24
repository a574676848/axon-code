import React from 'react';
import { CheckIcon, CloseIcon } from '../../icons';

/*
 * 控件基元：按钮、图标按钮、输入框、多行输入、开关、复选框。
 * 视觉规则集中在这里，业务组件只组合语义，不再各自拼装描边与聚焦效果。
 */

/** 控件通用描边：半像素发丝线，保证与背景分离但不产生重量。 */
const HAIRLINE = 'hairline border-border-input';

/** 键盘聚焦环：蓝色描边，只对键盘可见。 */
const FOCUS_RING = 'focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus';

type ButtonVariant = 'primary' | 'outline' | 'ghost' | 'danger' | 'link';
type ButtonSize = 'md' | 'sm';
/** 形状：常规圆角，或完整胶囊。 */
type ButtonShape = 'rounded' | 'pill';

const BUTTON_BASE =
  'btn-tactile inline-flex items-center justify-center gap-1.5 font-medium select-none whitespace-nowrap ' +
  'disabled:cursor-not-allowed disabled:opacity-40';

const BUTTON_SIZE: Record<ButtonSize, string> = {
  md: 'h-8 px-3 text-row',
  sm: 'h-7 px-2.5 text-row',
};

const BUTTON_VARIANT: Record<ButtonVariant, string> = {
  primary: 'bg-primary text-white shadow-btn-primary hover:bg-primary-hover',
  outline: `${HAIRLINE} bg-surface text-text-primary shadow-btn-outline hover:bg-hover hover:border-border-strong`,
  ghost: 'text-text-primary hover:bg-hover',
  danger: `${HAIRLINE} bg-surface text-danger shadow-btn-outline hover:bg-hover`,
  // 与 outline 同一几何，只把文字换成链接色：用于「有新版本」这类把人带出去、不在本界面完成的动作。
  link: `${HAIRLINE} bg-surface text-text-link shadow-btn-outline hover:bg-hover hover:border-border-strong`,
};

const BUTTON_SHAPE: Record<ButtonShape, Record<ButtonSize, string>> = {
  rounded: { md: 'rounded-md', sm: 'rounded-sm' },
  pill: { md: 'rounded-full', sm: 'rounded-full' },
};

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** 形状，默认常规圆角。 */
  shape?: ButtonShape;
}

export const Button: React.FC<ButtonProps> = ({
  variant = 'ghost',
  size = 'md',
  shape = 'rounded',
  className = '',
  type = 'button',
  ...rest
}) => (
  <button
    type={type}
    className={`${BUTTON_BASE} ${BUTTON_SIZE[size]} ${BUTTON_SHAPE[shape][size]} ${BUTTON_VARIANT[variant]} ${FOCUS_RING} ${className}`}
    {...rest}
  />
);

export interface IconButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  /** 无障碍名称，同时作为浏览器原生提示。 */
  label: string;
  size?: ButtonSize;
}

export const IconButton: React.FC<IconButtonProps> = ({
  label,
  size = 'sm',
  className = '',
  type = 'button',
  children,
  ...rest
}) => (
  <button
    type={type}
    aria-label={label}
    title={label}
    className={`btn-tactile inline-flex items-center justify-center shrink-0 text-text-secondary hover:text-text-primary hover:bg-hover disabled:cursor-not-allowed disabled:opacity-40 ${
      size === 'md' ? 'w-9 h-9 rounded-md' : 'w-7 h-7 rounded-sm'
    } ${FOCUS_RING} ${className}`}
    {...rest}
  >
    {children}
  </button>
);

/** 分段控件的一个候选项。 */
export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  /** 该项暂时不可选时的说明，同时作为原生提示。 */
  title?: string;
  disabled?: boolean;
}

export interface SegmentedControlProps<T extends string> {
  /** 无障碍名称。 */
  label: string;
  options: readonly SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  className?: string;
}

/**
 * 分段控件：同一维度的少量互斥选项。
 *
 * 与「一排并列按钮」不同，它是一枚整体控件：底槽给出组的存在，选中项是槽里
 * 唯一一块白底滑块。用于周期、维度、指标这类必须在同一行内切换的场景。
 *
 * 滑块是**等宽纯 transform 驱动**的实体胶囊（Glider）：
 * 宽度固定为 `calc((100% - 4px) / N)`，位移只走 `translate3d(index * 100%)`。
 * 因此既不重排也不拉伸，圆角在滑动全程恒定；外层托槽刻意不加 `overflow: hidden`，
 * 否则回弹时的圆角会被裁成直角。
 */
export const SegmentedControl = <T extends string>({
  label,
  options,
  value,
  onChange,
  className = '',
}: SegmentedControlProps<T>): React.ReactElement => {
  const count = options.length;
  const found = options.findIndex((option) => option.value === value);
  const activeIndex = found < 0 ? 0 : found;

  return (
    <div
      role="group"
      aria-label={label}
      className={`relative inline-grid grid-flow-col auto-cols-fr p-0.5 rounded-full bg-surface-sunken hairline border-border-hairline well-sm isolate ${className}`}
    >
      {count > 0 && (
        <span
          aria-hidden="true"
          data-segmented-glider=""
          className="pointer-events-none absolute left-0.5 top-0.5 bottom-0.5 rounded-full bg-surface shadow-lift will-change-transform"
          style={{
            width: `calc((100% - 4px) / ${count})`,
            transform: `translate3d(${activeIndex * 100}%, 0, 0)`,
            transition: 'transform 180ms var(--ease-spring-smooth)',
          }}
        />
      )}
      {options.map((option) => {
        const active = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            aria-pressed={active}
            disabled={option.disabled === true}
            title={option.title ?? option.label}
            onClick={() => onChange(option.value)}
            className={`relative z-10 h-6 px-2.5 rounded-full text-row whitespace-nowrap transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-40 ${
              active ? 'text-text-primary font-medium' : 'text-text-tertiary hover:text-text-primary'
            }`}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
};

export interface InputProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'size'> {
  /** 前置图标节点。 */
  icon?: React.ReactNode;
  /** 提供后会在尾部显示清空按钮。 */
  onClear?: () => void;
}

export const Input = React.forwardRef<HTMLInputElement, InputProps>(function Input(
  { icon, onClear, className = '', ...rest },
  ref,
) {
  return (
    <div
      className={`flex items-center gap-1.5 h-8 px-2.5 rounded-sm bg-surface ${HAIRLINE} focus-within:border-focus focus-within:shadow-[0_0_0_3px_var(--color-focus-soft)] transition-colors duration-100 ${className}`}
    >
      {icon !== undefined && <span className="text-text-tertiary shrink-0 flex">{icon}</span>}
      <input
        ref={ref}
        className="flex-1 min-w-0 bg-transparent border-none outline-none text-row text-text-primary placeholder:text-text-placeholder"
        {...rest}
      />
      {onClear !== undefined && (
        <IconButton label="清空" className="w-5 h-5 -mr-1" onClick={onClear}>
          <CloseIcon size={13} />
        </IconButton>
      )}
    </div>
  );
});

export type TextareaProps = React.TextareaHTMLAttributes<HTMLTextAreaElement>;

export interface NumberInputProps {
  /** 当前数值。 */
  value: number;
  /** 数值变化。 */
  onChange: (next: number) => void;
  /** 递增递减步长，默认 1。 */
  step?: number;
  /** 允许的最小值。 */
  min?: number;
  /** 允许的最大值。 */
  max?: number;
  /** 数值后缀单位，例如 px。 */
  unit?: string;
  /** 无障碍名称。 */
  label: string;
  /** 禁用态。 */
  disabled?: boolean;
  /** 附加类名，仅用于布局。 */
  className?: string;
}

/**
 * 数字控件：左右步进按钮夹一个数值输入。
 * 手输与步进共用同一套夹取规则，越界值被约束到 min 与 max。
 */
export const NumberInput: React.FC<NumberInputProps> = ({
  value,
  onChange,
  step = 1,
  min,
  max,
  unit,
  label,
  disabled = false,
  className = '',
}) => {
  const clamp = (next: number): number => {
    const lower = min === undefined ? next : Math.max(next, min);
    return max === undefined ? lower : Math.min(lower, max);
  };
  const canDecrease = !disabled && (min === undefined || value > min);
  const canIncrease = !disabled && (max === undefined || value < max);

  return (
    <div
      className={`flex items-center h-8 rounded-md bg-surface ${HAIRLINE} focus-within:border-border-selected transition-colors duration-100 ${className}`}
    >
      <button
        type="button"
        aria-label={`${label} 减少`}
        disabled={!canDecrease}
        onClick={() => onChange(clamp(value - step))}
        className="shrink-0 w-7 h-full flex items-center justify-center text-text-tertiary hover:text-text-primary disabled:opacity-30 disabled:cursor-not-allowed"
      >
        −
      </button>
      <input
        type="text"
        inputMode="numeric"
        aria-label={label}
        disabled={disabled}
        key={value}
        defaultValue={String(value)}
        onBlur={(event) => {
          const raw = event.currentTarget.value.trim();
          const parsed = Number(raw);
          const next = raw.length === 0 || !Number.isFinite(parsed) ? value : clamp(parsed);
          event.currentTarget.value = String(next);
          if (next !== value) onChange(next);
        }}
        onKeyDown={(event) => {
          if (event.key === 'Enter') event.currentTarget.blur();
          if (event.key === 'Escape') {
            event.currentTarget.value = String(value);
            event.currentTarget.blur();
          }
        }}
        className="flex-1 min-w-0 h-full bg-transparent border-none outline-none text-center text-row text-text-primary font-mono tabular-nums"
      />
      {unit !== undefined && (
        <span className="shrink-0 pr-1 text-xs text-text-tertiary">{unit}</span>
      )}
      <button
        type="button"
        aria-label={`${label} 增加`}
        disabled={!canIncrease}
        onClick={() => onChange(clamp(value + step))}
        className="shrink-0 w-7 h-full flex items-center justify-center text-text-tertiary hover:text-text-primary disabled:opacity-30 disabled:cursor-not-allowed"
      >
        +
      </button>
    </div>
  );
};

export interface CheckboxProps {
  /** 是否勾选。 */
  checked: boolean;
  /** 勾选状态变化。 */
  onChange: (next: boolean) => void;
  /** 无障碍名称。 */
  label: string;
  /** 禁用态。 */
  disabled?: boolean;
  /** 显示在勾选框旁的文本。 */
  children?: React.ReactNode;
}

/** 复选框：方框加对勾，勾选时使用主操作色。 */
export const Checkbox: React.FC<CheckboxProps> = ({
  checked,
  onChange,
  label,
  disabled = false,
  children,
}) => (
  <label className="flex items-center gap-2 min-w-0 cursor-pointer select-none">
    <input
      type="checkbox"
      className="peer sr-only"
      checked={checked}
      disabled={disabled}
      aria-label={label}
      onChange={() => onChange(!checked)}
    />
    <span
      aria-hidden="true"
      className={`shrink-0 w-4 h-4 rounded-xs border-[1.5px] flex items-center justify-center transition-colors duration-100 peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-focus peer-focus-visible:outline-offset-1 ${
        checked ? 'bg-primary border-primary text-white' : 'bg-surface border-border-input'
      } ${disabled ? 'opacity-40' : ''}`}
    >
      {checked && <CheckIcon size={11} />}
    </span>
    {children !== undefined && <span className="min-w-0 flex-1">{children}</span>}
  </label>
);

export const Textarea: React.FC<TextareaProps> = ({ className = '', ...rest }) => (
  <textarea
    className={`w-full p-2.5 rounded-md bg-surface ${HAIRLINE} text-row leading-relaxed text-text-primary placeholder:text-text-placeholder resize-none outline-none focus:border-focus focus:shadow-[0_0_0_3px_var(--color-focus-soft)] transition-colors duration-100 ${className}`}
    {...rest}
  />
);

export interface SwitchProps {
  /** 当前是否打开。 */
  checked: boolean;
  /** 状态变化回调。 */
  onChange: (next: boolean) => void;
  /** 禁用态。 */
  disabled?: boolean;
  /** 无障碍名称。 */
  label: string;
}

export const Switch: React.FC<SwitchProps> = ({ checked, onChange, disabled = false, label }) => (
  <button
    type="button"
    role="switch"
    aria-checked={checked}
    aria-label={label}
    disabled={disabled}
    onClick={() => onChange(!checked)}
    className={`relative w-9 h-5 rounded-full shrink-0 transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-40 ${FOCUS_RING} ${
      checked ? 'bg-primary' : 'bg-border-input'
    }`}
  >
    <span
      className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white shadow-sm transition-transform duration-150 ${
        checked ? 'translate-x-4' : 'translate-x-0'
      }`}
    />
  </button>
);

export interface KbdProps {
  /** 键名序列；多键组合按顺序给出，自动用 `+` 连接。 */
  keys: readonly string[];
  /** 附加类名，仅用于布局。 */
  className?: string;
}

/**
 * 快捷键角标：菜单项、按钮提示与命令面板共用的唯一键帽外观。
 *
 * 取 11px 角标档（不是 10px 微标签档：10px 只留给时间戳与坐标轴刻度），
 * 等宽字体保证 `Ctrl` 与 `Esc` 这类不同长度的键名横向对齐。
 * 图形本身不发声，名称由所在条目的文案承担，因此整块对辅助技术隐藏。
 */
export const Kbd: React.FC<KbdProps> = ({ keys, className = '' }) => (
  <span aria-hidden="true" className={`inline-flex shrink-0 items-center gap-0.5 ${className}`}>
    {keys.map((key, index) => (
      <React.Fragment key={key}>
        {index > 0 && <span className="text-micro text-text-tertiary">+</span>}
        <kbd className="px-1 py-px rounded-xs bg-surface-sunken hairline border-border font-mono text-micro leading-4 text-text-tertiary">
          {key}
        </kbd>
      </React.Fragment>
    ))}
  </span>
);
