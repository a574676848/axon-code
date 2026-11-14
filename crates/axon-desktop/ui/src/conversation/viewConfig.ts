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

/** 配置目录里的一条模型；把运行时回传的 provider/id 映射回展示名要用它。 */
export interface ConfigModelEntry {
  name: string;
  id: string;
  provider: string;
  /** 该模型的上下文窗口；未配置或非正数时为 null。 */
  contextWindow: number | null;
}

/**
 * 车道当前模型的权威事实。
 *
 * 网关只读该车道运行会话的**最新一轮**记录（一条 `ORDER BY sequence DESC LIMIT 1`），
 * 不遍历历史轮次；界面据此显示输入坞的真实模型。
 */
export interface LaneEffectiveModel {
  /** 该轮使用的 provider。 */
  provider: string;
  /** 该轮使用的 upstream 模型 ID。 */
  model: string;
  /** 用户当时选择的模型别名；旧记录没有别名时为 null 或省略。 */
  name?: string | null;
  /** 该轮实际请求的思考强度；未设置时为 null。 */
  reasoningEffort?: string | null;
}

/** 会话视图需要的配置取值。 */
export interface ResolvedConfig {
  /** 主模型的展示名；输入坞选择器用它。 */
  model: string;
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
   * 不在这里推导「供应商/模型」路由：同名模型可能有多条（不同供应商），运行时按
   * 渠道池、亲和与健康去选，配置里的第一条并不是本轮真正被请求的那一家。
   * 阶段行因此只认 provider 事件给出的真实路由，不拿这里的投影顶替。
   */
  return {
    model,
    reasoningEffort,
    contextWindow: matched?.context_window !== undefined && matched.context_window > 0
      ? matched.context_window
      : null,
    models,
    catalog: config.models.map(({ name, id, provider, context_window }) => ({
      name,
      id,
      provider,
      contextWindow: context_window > 0 ? context_window : null,
    })),
  };
}

/**
 * 把一轮真实运行的模型映射回配置里的展示名。
 *
 * 与运行时同一套匹配规则：先认记录里的别名（同一 provider 与 upstream id 必须同时
 * 对得上，别名才作数），再退回同 provider 同 id 精确匹配，其次「别名与 id 都对得上但
 * provider 变了」（渠道改名或迁移），最后只按 id。别名优先是因为同一 (provider, id)
 * 在配置里可能有多个别名，只按 id 取第一条会把用户当初选的那个别名显示成另一条。
 * 命中不了说明该模型已从配置移除，返回 null，由调用方回退到配置默认模型，那正是
 * 运行时会选的东西。
 */
export function modelLabelFor(
  catalog: ConfigModelEntry[],
  run: LaneEffectiveModel | null,
): string | null {
  if (run === null) return null;
  const name = run.name ?? null;
  if (name !== null) {
    const aliased = catalog.find(
      (entry) =>
        entry.name === name && entry.provider === run.provider && entry.id === run.model,
    );
    if (aliased !== undefined) return aliased.name;
  }
  const exact = catalog.find(
    (entry) => entry.provider === run.provider && entry.id === run.model,
  );
  if (exact !== undefined) return exact.name;
  if (name !== null) {
    const migrated = catalog.find((entry) => entry.name === name && entry.id === run.model);
    if (migrated !== undefined) return migrated.name;
  }
  return catalog.find((entry) => entry.id === run.model)?.name ?? null;
}

/** 按选择器（展示名、upstream id，或 `供应商/模型` 里的末段）找目录条目。 */
export function findModelEntry(
  catalog: ConfigModelEntry[],
  selector: string | null,
): ConfigModelEntry | null {
  if (selector === null) return null;
  const trimmed = selector.trim();
  if (trimmed === '') return null;
  const leaf = trimmed.split('/').pop() ?? trimmed;
  return (
    catalog.find((entry) => entry.name === trimmed) ??
    catalog.find((entry) => entry.id === trimmed) ??
    catalog.find((entry) => entry.name === leaf) ??
    catalog.find((entry) => entry.id === leaf) ??
    null
  );
}

/** 生效模型：展示名与它的上下文窗口必须成对给出，不能各读一份来源。 */
export interface EffectiveModel {
  /** 指标条与输入坞显示的模型展示名。 */
  label: string;
  /** 该模型的最大上下文；目录里查不到时回退配置默认模型的口径。 */
  contextWindow: number | null;
}

/**
 * 解析这次会话真正生效的模型与上下文窗口。
 *
 * 顺序与运行侧一致：目标固定模型 → 本会话显式选择 → 本会话真实运行面 → 配置默认。
 * 三者都拿不到时才用配置默认；窗口永远跟着最终的展示名走，避免「模型显示 A、
 * 窗口却是默认模型的数据」。
 */
export function resolveEffectiveModel(
  config: ResolvedConfig | null,
  fixedModel: string | null,
  explicitModel: string | null,
  laneModel: LaneEffectiveModel | null,
): EffectiveModel {
  const catalog = config?.catalog ?? [];
  const configModel = config?.model ?? '';
  const label =
    fixedModel ?? explicitModel ?? modelLabelFor(catalog, laneModel) ?? configModel;
  const matched = findModelEntry(catalog, label);
  return {
    label,
    contextWindow: matched === null ? (config?.contextWindow ?? null) : matched.contextWindow,
  };
}
