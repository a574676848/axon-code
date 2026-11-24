/*
 * 输入坞是否有未发送的正文。
 *
 * 切档会重启应用，草稿随之消失。这个标记是全局唯一的判据，供切档前的确认使用；
 * 它不进 React 状态：唯一写入方是输入坞的编辑状态，读取方只在切档那一刻问一次。
 */

let hasDraft = false;

/** 更新草稿标记；输入坞每次文本变化时调用。 */
export function setComposerHasDraft(value: boolean): void {
  hasDraft = value;
}

/** 当前是否有未发送的正文。 */
export function composerHasDraft(): boolean {
  return hasDraft;
}
