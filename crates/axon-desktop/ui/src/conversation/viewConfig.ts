/**
 * 会话视图的配置投影。
 *
 * 指标条与输入坞取的是同一批后端配置：主模型、推理等级与上下文窗口。
 * 取法只有这一份（含固定模型对主模型的覆盖规则），因此独立成模块，
 * 视图只负责读配置与写回配置。
 */

import type { ConfigReadResult } from '../types';
import { isEmbeddingModel } from '../modelCatalog';
import type { SessionRunFacets } from './transcript/state';

/** 后端配置里与指标条相关的键。 */
export const KEY_DEFAULT_MODEL = 'agent.default_model';
export const KEY_REASONING_EFFORT = 'agent.reasoning_effort';

/** 配置目录里的一条模型；把运行时回传的 provider/id 映射回展示名要用它。 */
export interface ConfigModelEntry {
  name: string;
  id: string;
  provider: string;
}

/** 会话视图需要的配置取值。 */
export interface ResolvedConfig {
  /** 主模型的展示名；输入坞选择器用它。 */
  model: string;
  /** 主模型的完整路由（`供应商/模型 id`）；工作状态行用它，缺路由时回落到展示名。 */
  route: string;
  reasoningEffort: string;
  contextWindow: number | null;
  /** 可选模型清单：取配置目录里的模型名，去重后保持配置顺序。 */
  models: string[];
  /** 配置目录全量条目（含 id 与 provider），供真实运行面反查展示名。 */
  catalog: ConfigModelEntry[];
}

/** 从配置读取结果里取出指标条与输入坞需要的项。 */
export function resolveConfig(
  config: ConfigReadResult | null,
  fixedModel?: string | null,
): ResolvedConfig | null {
  if (config === null) return null;
  const model = fixedModel ?? String(config.flat_values[KEY_DEFAULT_MODEL] ?? '');
  const reasoningEffort = String(config.flat_values[KEY_REASONING_EFFORT] ?? '');
  const leaf = model.split('/').pop() ?? model;
  const matched = config.models.find(
    (candidate) =>
      candidate.name === model ||
      candidate.id === model ||
      candidate.name === leaf ||
      candidate.id === leaf,
  );
  const models = Array.from(new Set(config.models
    .filter((candidate) => !isEmbeddingModel(candidate))
    .map((candidate) => candidate.name)));
  /*
   * 路由：配置里已经写成 `供应商/模型` 时原样沿用（用户明确指定了走哪一家），
   * 否则按配置目录里那一条补全。同名模型可能有多条（不同供应商），这里取第一条，
   * 与运行时的解析规则一致。
   */
  const explicitRoute = model.includes('/') ? model : null;
  return {
    model,
    route: explicitRoute ?? (matched === undefined ? model : `${matched.provider}/${matched.id}`),
    reasoningEffort,
    contextWindow: matched?.context_window !== undefined && matched.context_window > 0
      ? matched.context_window
      : null,
    models,
    catalog: config.models.map(({ name, id, provider }) => ({ name, id, provider })),
  };
}

/**
 * 把一轮真实运行的模型映射回配置里的展示名。
 *
 * 与运行时同一套匹配规则：同 provider 同 upstream id 精确优先，其次只按 id
 * （provider 改名或迁移时的兜底）。命中不了说明该模型已从配置移除，返回 null，
 * 由调用方回退到配置默认模型，那正是运行时会选的东西。
 */
export function modelLabelFor(
  catalog: ConfigModelEntry[],
  run: SessionRunFacets | null,
): string | null {
  if (run === null) return null;
  const exact = catalog.find(
    (entry) => entry.provider === run.provider && entry.id === run.model,
  );
  if (exact !== undefined) return exact.name;
  return catalog.find((entry) => entry.id === run.model)?.name ?? null;
}
