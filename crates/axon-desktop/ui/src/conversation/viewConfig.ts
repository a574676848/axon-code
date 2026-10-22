/**
 * 会话视图的配置投影。
 *
 * 指标条与输入坞取的是同一批后端配置：主模型、推理等级与上下文窗口。
 * 取法只有这一份（含固定模型对主模型的覆盖规则），因此独立成模块，
 * 视图只负责读配置与写回配置。
 */

import type { ConfigReadResult } from '../types';
import { isEmbeddingModel } from '../modelCatalog';

/** 后端配置里与指标条相关的键。 */
export const KEY_DEFAULT_MODEL = 'agent.default_model';
export const KEY_REASONING_EFFORT = 'agent.reasoning_effort';

/** 会话视图需要的配置取值。 */
export interface ResolvedConfig {
  model: string;
  reasoningEffort: string;
  contextWindow: number | null;
  /** 可选模型清单：取配置目录里的模型名，去重后保持配置顺序。 */
  models: string[];
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
  return {
    model,
    reasoningEffort,
    contextWindow: matched?.context_window !== undefined && matched.context_window > 0
      ? matched.context_window
      : null,
    models,
  };
}
