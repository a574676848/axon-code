/**
 * 输入坞的高度策略探测。
 *
 * `field-sizing: content` 让输入框按内容自己撑高，因此不需要在每次输入时读
 * `scrollHeight` 再把高度写回去。读 `scrollHeight` 是强制同步回流：先前的样式
 * 必须当场结算，才能拿到这个数；每敲一个字付一次布局代价，换来的信息 CSS 本来就有。
 *
 * 老引擎没有这条属性时仍然走量高度那条路，保证功能不缺失。
 */
export const FIELD_SIZING_SUPPORTED: boolean =
  typeof CSS !== 'undefined' &&
  typeof CSS.supports === 'function' &&
  CSS.supports('field-sizing', 'content');
