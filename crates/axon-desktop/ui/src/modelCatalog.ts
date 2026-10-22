import type { ModelItem } from './types';

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
