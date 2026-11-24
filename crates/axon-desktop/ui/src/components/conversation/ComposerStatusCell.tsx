import React from 'react';

/*
 * 输入坞上沿状态条的外壳。
 *
 * 语音录音条与工作状态行说的是同一件事的两种语气（正在收声、正在干活），
 * 因此高度、圆角、底色、描边、阴影、内部节奏与入场动画只在这里定义一次：
 * 两条同时在场时读起来是同一列，而不是两块各写各的横条。
 *
 * 外壳只负责几何与节奏；左端标识、中间文案与右端提示由调用方给，
 * 因为两条各自要说的内容并不相同。
 *
 * 底边的「还在动」提示分两层：一条恒定发丝轨道，加一束往返的墨色光泽。
 * 用中性墨色而不是强调色：这一条是常驻读数条，比正文更抢眼就不对了。
 */

export interface ComposerStatusCellProps
  extends Omit<React.HTMLAttributes<HTMLDivElement>, 'children'> {
  /** 左端状态标识：工作脉搏、声波加品牌点。 */
  glyph: React.ReactNode;
  /** 中间内容区：主文案、等宽读数与弹性说明。 */
  children: React.ReactNode;
  /** 右端提示，例如 `按 ESC 中断`、`Esc 关闭`；不给则不占位。 */
  hint?: string;
  /** 底边的不确定进度光泽；只有工作状态条需要。 */
  sweeping?: boolean;
  /** 外边距与必要的排布差异；间距由调用方给，外壳不自带，避免同类工具类互相压。 */
  className?: string;
}

export const ComposerStatusCell: React.FC<ComposerStatusCellProps> = ({
  glyph,
  children,
  hint,
  sweeping = false,
  className = '',
  ...rest
}) => (
  <div
    className={`popover-up-in relative flex h-10 items-center gap-2 overflow-hidden rounded-md bg-surface px-2.5 shadow-panel hairline border-border ${className}`}
    {...rest}
  >
    <span className="flex shrink-0 items-center gap-1.5">{glyph}</span>
    <span className="flex min-w-0 flex-1 items-baseline gap-2">{children}</span>
    {hint !== undefined && <span className="shrink-0 text-meta text-text-tertiary">{hint}</span>}
    {sweeping && (
      <>
        <span
          aria-hidden="true"
          className="working-rail-track pointer-events-none absolute inset-x-0 bottom-0 h-px"
        />
        <span
          aria-hidden="true"
          className="working-sweep pointer-events-none absolute bottom-0 -left-1/3 h-[2px] w-1/3 rounded-full bg-text-primary/25"
        />
      </>
    )}
  </div>
);
