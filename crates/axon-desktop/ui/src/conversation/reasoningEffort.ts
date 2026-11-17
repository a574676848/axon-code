/**
 * 思考强度档位。
 *
 * 取值与后端配置枚举逐一对应，这里是输入坞选择器与「缓存值是否仍然合法」的
 * 唯一清单：增删档位只改这一处。
 */
export const REASONING_EFFORTS = [
  'none',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
] as const;

/** 取值是否落在后端接受的档位内。 */
export function isReasoningEffort(value: string | null | undefined): value is string {
  if (value === null || value === undefined) return false;
  return (REASONING_EFFORTS as readonly string[]).includes(value);
}
