/*
 * 输入坞的内置指令。
 *
 * 与「已安装技能」不是一回事：技能是发给模型的正文，内置指令由界面自己执行
 * （或转给运行态接口），不进入会话。清单只有一处：候选菜单、正文识别与提交拦截
 * 都读这一份，因此新增一条指令不需要在三个地方各改一遍。
 */

/** 一条内置指令。 */
export interface BuiltinCommand {
  /** 指令名，不含斜杠。 */
  name: string;
  /** 候选菜单右侧的一行说明。 */
  description: string;
}

/** 全部内置指令。 */
export const BUILTIN_COMMANDS: readonly BuiltinCommand[] = [
  { name: 'mcp', description: '重新加载 MCP 服务器列表' },
];

/**
 * 整条正文恰好是一条内置指令时返回它。
 *
 * 只认「正文就是一个指令」（尾部空白不算）：`/mcp` 是本机动作，而 `/mcp 看看这个`
 * 是一句想让模型读的话，两者不能混为一谈。
 */
export function matchBuiltinCommand(text: string): BuiltinCommand | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith('/')) return null;
  const name = trimmed.slice(1).trim();
  return BUILTIN_COMMANDS.find((command) => command.name === name) ?? null;
}
