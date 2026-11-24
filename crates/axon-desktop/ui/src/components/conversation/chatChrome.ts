/*
 * 对话域的几何与层级常量。
 *
 * 全部集中在这里，是为了让「同一类东西只有一个取值」成为可检查的事实：
 * 圆角落一处、层级落三档、行高落一处。
 */

/** 用户气泡圆角：上两角与右下角同取大圆角，左下角收成 4px 指向发送者。 */
export const BUBBLE_RADIUS = '18px 18px 4px 18px';

/*
 * z 轴只有三档。
 *
 * 1) 正文浮层：过渡面、回到最新、消息导航条、宽度手柄，压在正文之上但不拦菜单；
 * 2) 菜单浮层：候选菜单、模型选择、行悬浮卡，与 HoverCard 同档（z-[100]）；
 * 3) 模态：灯箱与 Dialog，与 overlays 的模态同档（z-[1000]）。
 * 新增浮层先归到这三档里，不再出现 z-25 / z-50 / z-[90] 这类一次性取值。
 */
export const Z_CONTENT_OVERLAY = 'z-30';
export const Z_CONTENT_RAIL = 'z-20';
export const Z_MENU = 'z-[100]';
/** 菜单的点外关闭层：压在菜单之下、正文之上，只负责收菜单。 */
export const Z_MENU_SCRIM = 'z-[99]';
export const Z_MODAL = 'z-[1000]';

/** 消息页脚高度：时间与技术按钮共用同一行。 */
export const MESSAGE_FOOTER_HEIGHT = 'h-7';
/** 消息操作按钮命中区：28px 见方，图形 14px。 */
export const MESSAGE_ACTION_SIZE = 'w-7 h-7';
/** 消息时间：12px，等宽数字。 */
export const MESSAGE_TIME_CLASS = 'px-1 text-meta tabular-nums text-text-tertiary';
