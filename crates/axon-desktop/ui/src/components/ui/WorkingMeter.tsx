import React from 'react';

/*
 * 工作脉搏：输入坞工作状态行的进行标识。
 *
 * 三根依次起伏的细柱，读作「正在产出」而不是「在等某个过程结束」，因此不用转圈：
 * 转圈表达的是未知时长的等待，而这一行的时间就在旁边按秒走。
 *
 * 颜色一律取当前文本色（`currentColor`），不引入强调色：状态行是常驻读数条，
 * 蓝调会让它比正文更抢眼。柱高差异靠 opacity 分档，动画只动 `transform: scaleY`
 * 与 `opacity`，`prefers-reduced-motion` 下由全局规则落到终态。
 *
 * 全站只有这一份实现（样式在 `tokens.css` 的 `.working-meter`），
 * 语音录音条用声波、工作状态行用脉搏，两者不混用。
 */
export const WorkingMeter: React.FC<{ className?: string }> = ({ className = '' }) => (
  <span className={`working-meter ${className}`} aria-hidden="true">
    <span className="working-bar" />
    <span className="working-bar" />
    <span className="working-bar" />
  </span>
);
