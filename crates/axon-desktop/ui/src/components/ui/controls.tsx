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

type ButtonVariant = 'primary' | 'outline' | 'ghost' | 'danger';
type ButtonSize = 'md' | 'sm';
/** 形状：常规圆角，或完整胶囊。 */
type ButtonShape = 'rounded' | 'pill';

const BUTTON_BASE =
  'inline-flex items-center justify-center gap-1.5 font-medium select-none whitespace-nowrap ' +
  'transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-40';

const BUTTON_SIZE: Record<ButtonSize, string> = {
  md: 'h-8 px-3 text-row',
  sm: 'h-7 px-2.5 text-row',
};

const BUTTON_VARIANT: Record<ButtonVariant, string> = {
  primary: 'bg-primary text-white hover:bg-primary-hover',
  outline: `${HAIRLINE} bg-surface text-text-primary hover:bg-hover`,
  ghost: 'text-text-primary hover:bg-hover',
  danger: `${HAIRLINE} bg-surface text-danger hover:bg-hover`,
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
    className={`inline-flex items-center justify-center shrink-0 text-text-secondary hover:text-text-primary hover:bg-hover transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-40 ${
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
 * 唯一一块白底，因此「当前选的是哪一个」在视觉上是一句话。用于周期、维度、
 * 指标这类必须在同一行内切换的场景。
 */
export const SegmentedControl = <T extends string>({
  label,
  options,
  value,
  onChange,
  className = '',
}: SegmentedControlProps<T>): React.ReactElement => (
  <div
    role="group"
    aria-label={label}
    className={`inline-flex items-center gap-0.5 p-0.5 rounded-full bg-surface-sunken hairline border-border-hairline ${className}`}
  >
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
          className={`h-6 px-2.5 rounded-full text-row whitespace-nowrap transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-40 ${
            active
              ? 'bg-surface text-text-primary font-medium shadow-panel'
              : 'text-text-tertiary hover:text-text-primary hover:bg-hover'
          }`}
        >
          {option.label}
        </button>
      );
    })}
  </div>
);

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
      className={`flex items-center gap-1.5 h-8 px-2.5 rounded-sm bg-surface ${HAIRLINE} focus-within:border-focus transition-colors duration-100 ${className}`}
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
    className={`w-full p-2.5 rounded-sm bg-surface ${HAIRLINE} text-row leading-relaxed text-text-primary resize-none outline-none focus:border-focus transition-colors duration-100 ${className}`}
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
