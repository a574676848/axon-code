import type { MemoryKind, MemoryWriteInput } from '../types';

/** 五值记忆类型的展示元数据；顺序即界面分栏顺序。 */
export const MEMORY_KINDS: Array<{ value: MemoryKind; label: string; scope: 'personal' | 'workspace' }> = [
  { value: 'profile', label: '画像', scope: 'personal' },
  { value: 'preference', label: '偏好', scope: 'personal' },
  { value: 'experience', label: '经验', scope: 'workspace' },
  { value: 'knowledge', label: '知识', scope: 'workspace' },
  { value: 'procedure', label: '流程', scope: 'workspace' },
];

/** 记忆类型允许出现在 JSON 录入里的全部字段。 */
export const MEMORY_ENTRY_FIELDS = ['memoryKind', 'summary', 'evidence', 'workspacePath', 'supersedesMemoryIds'];

/** 记忆的归属作用域：Personal 只承载画像与偏好。 */
export const scopeOfKind = (kind: MemoryKind): 'personal' | 'workspace' =>
  MEMORY_KINDS.find((item) => item.value === kind)?.scope ?? 'workspace';

/** 记忆类型的中文名；界面不直接输出英文枚举。 */
export const memoryKindLabel = (kind: MemoryKind): string =>
  MEMORY_KINDS.find((item) => item.value === kind)?.label ?? kind;

/** 记忆类型是否为 Personal 类型。 */
export const isPersonalKind = (kind: MemoryKind): boolean => scopeOfKind(kind) === 'personal';

/** 记忆类型是否为 Workspace（仓库）类型。 */
export const isWorkspaceKind = (kind: MemoryKind): boolean => scopeOfKind(kind) === 'workspace';

export const isMemoryKind = (value: unknown): value is MemoryKind =>
  typeof value === 'string' && MEMORY_KINDS.some((item) => item.value === value);

/** JSON 录入解析结果；错误逐条给出，便于界面直接展示。 */
export interface MemoryDraftResult {
  entries: MemoryWriteInput[];
  errors: string[];
}

/**
 * 把 JSON 录入文本规范成待提交条目。
 *
 * 支持单条对象、对象数组与 `records` 包装体三种形态；类型与作用域的约束与
 * Gateway 完全一致，先在本地拦下再提交，避免把用户输入送到服务端才发现。
 */
export const draftEntriesFromJson = (text: string): MemoryDraftResult => {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (failure) {
    return { entries: [], errors: [`JSON 解析失败：${failure instanceof Error ? failure.message : '未知错误'}`] };
  }
  const raw = Array.isArray(parsed)
    ? parsed
    : typeof parsed === 'object' && parsed !== null && Array.isArray((parsed as { records?: unknown }).records)
      ? (parsed as { records: unknown[] }).records
      : [parsed];
  if (raw.length === 0) return { entries: [], errors: ['JSON 中没有可写入的记忆条目'] };

  const errors: string[] = [];
  const entries: MemoryWriteInput[] = [];
  raw.forEach((item, index) => {
    const position = `第 ${index + 1} 条`;
    if (typeof item !== 'object' || item === null || Array.isArray(item)) {
      errors.push(`${position}：必须是对象`);
      return;
    }
    const source = item as Record<string, unknown>;
    const unknownKeys = Object.keys(source).filter((key) => !MEMORY_ENTRY_FIELDS.includes(key));
    if (unknownKeys.length > 0) {
      errors.push(`${position}：不支持字段 ${unknownKeys.join('、')}`);
      return;
    }
    if (!isMemoryKind(source.memoryKind)) {
      errors.push(`${position}：memoryKind 必须是 ${MEMORY_KINDS.map((entry) => entry.value).join('、')}`);
      return;
    }
    if (typeof source.summary !== 'string' || source.summary.trim().length === 0) {
      errors.push(`${position}：summary 不能为空`);
      return;
    }
    if (typeof source.evidence !== 'string' || source.evidence.trim().length === 0) {
      errors.push(`${position}：evidence 不能为空`);
      return;
    }
    const workspacePath = typeof source.workspacePath === 'string' ? source.workspacePath.trim() : undefined;
    if (isWorkspaceKind(source.memoryKind) && (workspacePath === undefined || workspacePath.length === 0)) {
      errors.push(`${position}：仓库记忆必须提供 workspacePath 文件夹目录`);
      return;
    }
    if (isPersonalKind(source.memoryKind) && workspacePath !== undefined) {
      errors.push(`${position}：画像与偏好属于 Personal 记忆，不能提供 workspacePath`);
      return;
    }
    // id 超出 JS 安全整数区间：JSON 里允许写十进制字符串，也兼容未丢精度的数字。
    const supersedes = Array.isArray(source.supersedesMemoryIds)
      ? source.supersedesMemoryIds
          .map((value) => (typeof value === 'number' ? String(value) : value))
          .filter((value): value is string => typeof value === 'string' && /^[1-9][0-9]*$/.test(value))
      : undefined;
    entries.push({
      memoryKind: source.memoryKind,
      summary: source.summary.trim(),
      evidence: source.evidence.trim(),
      workspacePath,
      supersedesMemoryIds: supersedes,
    });
  });
  return errors.length > 0 ? { entries: [], errors } : { entries, errors };
};
