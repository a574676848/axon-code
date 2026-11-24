import type { DiscoveredModel, ModelInput, ModelItem, ProviderItem } from './types';

/*
 * 模型用途判定。
 *
 * `supports_embedding` 是权威值；供应商目录偶尔漏标（例如 text-embedding-v4
 * 仍标成 false），因此名称里带 embedding 的也按向量模型处理。
 * 对话模型选择与向量模型选择共用这一条规则，避免两处过滤口径分叉。
 */
const EMBEDDING_NAME = /embedding/i;

/** 该模型是否只用于向量检索，不能作为对话模型。 */
export function isEmbeddingModel(
  model: Pick<ModelItem, 'id' | 'name' | 'supports_embedding'>,
): boolean {
  return (
    model.supports_embedding ||
    EMBEDDING_NAME.test(model.id) ||
    EMBEDDING_NAME.test(model.name)
  );
}

/**
 * 供应商的启用表。
 *
 * 停用供应商的模型仍在配置里，但运行侧一条路由都不会走：CLI 的 `axon models`、
 * TUI 的 `/model` 与桌面端的选择器都必须把它们排除，否则停用一个供应商之后
 * 界面上还能选到一组注定失败的模型。
 *
 * 配置里查不到该供应商的模型同样按停用处理：模型指向一个不存在的供应商是坏配置，
 * 不该出现在可选项里。
 */
export function isProviderEnabled(
  providers: readonly Pick<ProviderItem, 'name' | 'enabled'>[],
  providerName: string,
): boolean {
  return providers.some((provider) => provider.name === providerName && provider.enabled);
}

/**
 * 对话模型候选：向量模型之外的、所属供应商已启用的模型。
 *
 * `providers` 为 null 表示这份载荷里没有供应商事实（旧回执或读取失败），
 * 此时只按用途过滤，不把整份候选清空：宁可多列一条，也不要让选择器空掉。
 */
export function selectableChatModels(
  models: readonly ModelItem[],
  providers: readonly Pick<ProviderItem, 'name' | 'enabled'>[] | null,
): ModelItem[] {
  return models.filter(
    (model) =>
      !isEmbeddingModel(model) &&
      (providers === null || isProviderEnabled(providers, model.provider)),
  );
}

/** 向量模型候选：与对话候选同一套供应商启用判定，只换用途。 */
export function selectableEmbeddingModels(
  models: readonly ModelItem[],
  providers: readonly Pick<ProviderItem, 'name' | 'enabled'>[] | null,
): ModelItem[] {
  return models.filter(
    (model) =>
      isEmbeddingModel(model) &&
      (providers === null || isProviderEnabled(providers, model.provider)),
  );
}

/** 新建模型行的初始值；归属供应商由调用方补。 */
export const EMPTY_MODEL: Omit<ModelInput, 'provider'> = {
  name: '',
  id: '',
  context_window: 128000,
  max_output_tokens: 8192,
  supports_vision: true,
  supports_embedding: false,
  embedding_dimension: 0,
  requires_reasoning_content_on_assistant_messages: false,
};

/*
 * 把上游勾选的候选并入草稿。
 *
 * 判重只能按 name：后端 provider_profile_upsert_at 以 name 作为同供应商下的唯一键，
 * 保存时按 name 做整体替换，本次没提交的 name 会被删掉。按 id 去重会把
 * 「同一个上游 ID 配多个路由名」这种合法配置塌缩成一条，另一条随后被静默删除。
 */
export function mergeDiscoveredModels(
  models: ModelInput[],
  picked: DiscoveredModel[],
  providerName: string,
): ModelInput[] {
  const next = [...models];
  for (const candidate of picked) {
    const name = (candidate.name ?? candidate.id).trim();
    if (name.length === 0) continue;
    if (next.some((model) => model.name.trim() === name)) continue;
    next.push({
      ...EMPTY_MODEL,
      name,
      id: candidate.id,
      provider: providerName,
      context_window: candidate.context_window ?? EMPTY_MODEL.context_window,
      max_output_tokens: candidate.max_output_tokens ?? EMPTY_MODEL.max_output_tokens,
    });
  }
  return next;
}
