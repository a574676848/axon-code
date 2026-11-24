import type { DiscoveredModel, ModelInput, ModelItem } from './types';

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
