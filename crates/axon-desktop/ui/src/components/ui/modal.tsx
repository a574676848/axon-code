import React, { useContext, useEffect, useRef } from 'react';
import { CloseIcon } from '../../icons';
import { Button, IconButton } from './controls';
import { useExitLifecycle } from './useExitLifecycle';

/*
 * 模态与对话框：全站浮层的唯一承载实现。
 *
 * 与菜单族（`overlays.tsx`）分开成文件，是因为两者职责不同：
 * 菜单是「贴着锚点的短生命周期浮层」，模态是「独占一屏并在关闭时走退场动画」。
 * 放在同一份文件里会让两套完全不同的几何与生命周期互相干扰阅读。
 */

/**
 * 模态嵌套深度。
 *
 * 用 context 而不是查询 DOM：查询会把「当前这个弹窗」自己也数进去，
 * 弹窗任意一次重渲染都会误判为嵌套并把遮罩摘掉。context 只统计祖先，
 * 子弹窗读到 1，父弹窗读到 0，判定稳定。
 */
const ModalDepthContext = React.createContext(0);

export interface ModalProps {
  /** 是否打开。 */
  open: boolean;
  /** 关闭回调。 */
  onClose: () => void;
  /** 标题。 */
  title: string;
  /** 标题右侧操作区。 */
  actions?: React.ReactNode;
  /** 顶部形态：独立标题栏、右上角浮动关闭，或无内置头部由子面板自行排布。 */
  variant?: 'bar' | 'floating' | 'headless';
  /** 面板尺寸类名。 */
  panelClassName?: string;
  /** 内容。 */
  children: React.ReactNode;
}

export const Modal: React.FC<ModalProps> = ({
  open,
  onClose,
  title,
  actions,
  variant = 'bar',
  panelClassName = 'w-full max-w-4xl h-[640px]',
  children,
}) => {
  const dialogRef = useRef<HTMLDivElement>(null);
  /** 祖先弹窗深度：0 表示这是最外层，需要画遮罩。 */
  const depth = useContext(ModalDepthContext);
  /*
   * 退场期内容冻结。
   *
   * 父级把 open 置为 false 时往往同时清掉数据（例如把失败原因置 null），
   * 照单全收会让面板在淡出的 110ms 里先塌成一个空壳，读起来像一次塌陷。
   * 因此收起期间保持上一份内容，只让透明度与缩放变化。
   */
  const frozen = useRef(children);
  if (open) frozen.current = children;
  const lifecycle = useExitLifecycle(open);

  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        const dialogs = document.querySelectorAll('[role="dialog"][aria-modal="true"]');
        if (dialogs.item(dialogs.length - 1) !== dialogRef.current) return;
        event.preventDefault();
        onClose();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [open, onClose]);

  if (!lifecycle.mounted) return null;

  /*
   * 嵌套弹窗不再叠第二层遮罩：父弹窗已经把背景压暗，
   * 再叠一层会让整窗变成近半黑并两次模糊。深度来自 context，只统计祖先，
   * 因此当前弹窗自己的重渲染不会把遮罩摘掉。
   */
  const nested = depth > 0;
  const state = lifecycle.state;

  return (
    <ModalDepthContext.Provider value={depth + 1}>
      <div
        className={`fixed inset-0 z-[1000] flex items-center justify-center p-4 sm:p-6 ${
          state === 'closed' ? 'pointer-events-none' : ''
        }`}
      >
      {!nested && (
        <div
          data-state={state}
          className="modal-static-backdrop overlay-scrim-in absolute inset-0"
          onClick={onClose}
          aria-hidden="true"
        />
      )}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        data-state={state}
        /*
         * `max-h-full` 是硬性约束：外层是 `fixed inset-0` 加内边距的居中容器，
         * 没有它时面板会按内容长高、顶出上下边界（标题被裁掉、列表既看不全也滚不动），
         * 内层的 `flex-1 min-h-0` 也就没有可收缩的高度可用。
         *
         * `overflow-clip` 而不是 `overflow-hidden`：`hidden` 仍然是一个可滚动容器，
         * 键盘把焦点移到弹窗内部控件时，浏览器会为「把焦点滚进视野」而滚动它，
         * 整块内容被顶出面板、底栏下方留出空白（内容越多偏得越远）。
         * `clip` 让面板彻底不是滚动容器，浏览器再也没有可滚动的对象。
         */
        className={`modal-spring-surface overlay-panel-in relative flex flex-col overflow-clip bg-surface rounded-lg shadow-modal hairline border-border-hairline max-h-full min-h-0 ${panelClassName}`}
      >
        {variant === 'bar' && (
          <div className="shrink-0 px-5 py-3 flex items-center justify-between gap-4 hairline-b border-border-hairline">
            <h2 className="text-body font-semibold text-text-primary">{title}</h2>
            <div className="shrink-0 flex items-center gap-1.5">
              {actions}
              <IconButton label="关闭" onClick={onClose} className="ml-0.5">
                <CloseIcon size={14} />
              </IconButton>
            </div>
          </div>
        )}
        {variant === 'floating' && (
          <div className="absolute top-3 right-3 z-20 flex items-center gap-2">
            {actions}
            <IconButton label="关闭" onClick={onClose}>
              <CloseIcon size={14} />
            </IconButton>
          </div>
        )}
        <div className="flex-1 min-h-0 flex overflow-clip">
          {open ? children : frozen.current}
        </div>
      </div>
      </div>
    </ModalDepthContext.Provider>
  );
};

/** 对话框宽度档位。 */
export type DialogSize = 'sm' | 'md' | 'lg';

const DIALOG_SIZE: Record<DialogSize, string> = {
  sm: 'w-full max-w-md',
  md: 'w-full max-w-lg',
  lg: 'w-full max-w-2xl',
};

/** 右下角主操作。 */
export interface DialogAction {
  /** 按钮文案。 */
  label: string;
  /** 忙碌时的替代文案。 */
  busyLabel?: string;
  /** 前置图标。 */
  icon?: React.ReactNode;
  /** 是否忙碌。 */
  busy?: boolean;
  /** 是否禁用。 */
  disabled?: boolean;
  /** 点击回调。 */
  onClick: () => void;
}

export interface DialogProps {
  /** 是否打开。 */
  open: boolean;
  /** 关闭回调。 */
  onClose: () => void;
  /** 标题。 */
  title: string;
  /** 宽度档位，默认中号。 */
  size?: DialogSize;
  /** 右下角主操作。 */
  primary?: DialogAction;
  /** 主操作左侧的次操作，默认描边样式。 */
  secondary?: DialogAction & { danger?: boolean };
  /** 底部左侧的一行说明。 */
  hint?: string;
  /** 正文。 */
  children: React.ReactNode;
}

/**
 * 统一的对话框形态：关闭只走右上角图标，主操作固定右下角。
 *
 * 所有弹窗都必须经由这里拼装，不允许各自摆放按钮位置，
 * 以保证后续新增弹窗与既有弹窗的交互完全一致。
 */
export const Dialog: React.FC<DialogProps> = ({
  open,
  onClose,
  title,
  size = 'md',
  primary,
  secondary,
  hint,
  children,
}) => {
  const hasFooter = primary !== undefined || secondary !== undefined || hint !== undefined;
  return (
    <Modal open={open} onClose={onClose} title={title} panelClassName={DIALOG_SIZE[size]}>
      <div className="w-full min-h-0 flex flex-col">
        <div className="flex-1 min-h-0 overflow-y-auto px-5 py-4 space-y-3.5">{children}</div>
        {hasFooter && (
          <div className="shrink-0 flex items-center gap-2 px-5 py-3 bg-surface-sunken/30 hairline-t border-border-hairline">
            {hint !== undefined && (
              <span className="flex-1 min-w-0 truncate text-xs text-text-tertiary" title={hint}>
                {hint}
              </span>
            )}
            <div className="ml-auto shrink-0 flex items-center gap-2">
              {secondary !== undefined && (
                <Button
                  variant={secondary.danger === true ? 'danger' : 'outline'}
                  size="md"
                  disabled={secondary.disabled === true || secondary.busy === true}
                  onClick={secondary.onClick}
                >
                  {secondary.icon}
                  {secondary.busy === true && secondary.busyLabel !== undefined
                    ? secondary.busyLabel
                    : secondary.label}
                </Button>
              )}
              {primary !== undefined && (
                <Button
                  variant="primary"
                  size="md"
                  disabled={primary.disabled === true || primary.busy === true}
                  onClick={primary.onClick}
                >
                  {primary.icon}
                  {primary.busy === true && primary.busyLabel !== undefined
                    ? primary.busyLabel
                    : primary.label}
                </Button>
              )}
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
};
