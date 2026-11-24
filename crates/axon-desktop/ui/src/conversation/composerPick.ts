/**
 * 输入坞的显式选择缓存。
 *
 * 用户在输入坞改过模型或思考强度之后，这一份选择会落在客户端状态文件
 * （`desktop-state.v2.json`）里，新开会话时优先沿用它。模型按**展示名**记录：
 * 名称是用户在输入坞里认得出的那一个，`id` 会随供应商版本漂移。
 *
 * 历史会话不继承这份缓存：它自己的权威模型来自该车道最新一轮的运行记录。
 */

import { isReasoningEffort } from './reasoningEffort';

/** 一份输入坞选择；空值表示用户从未选过。 */
export interface ComposerPick {
  /** 模型展示名，即配置里 `[[models]].name`。 */
  model: string | null;
  /** 思考强度档位。 */
  reasoningEffort: string | null;
}

/** 没有缓存时的取值。 */
export const EMPTY_COMPOSER_PICK: ComposerPick = { model: null, reasoningEffort: null };

/** 空白按没有选过处理：状态文件与手改值都不该留下一个空串选择。 */
function clean(value: string | null | undefined): string | null {
  const trimmed = (value ?? '').trim();
  return trimmed === '' ? null : trimmed;
}

/**
 * 新会话可继承的选择。
 *
 * 模型名称必须仍在本配置的模型目录里，否则整份缓存作废，回到
 * `agent.default_model` 与 `agent.reasoning_effort`：一个已经从配置里消失的名称
 * 下发给运行侧只会失败，不该让新会话继承它。只选过强度时模型仍由配置默认值决定，
 * 强度照旧沿用。
 */
export function inheritedComposerPick(
  modelNames: readonly string[],
  cache: ComposerPick,
): ComposerPick {
  const model = clean(cache.model);
  const effort = clean(cache.reasoningEffort);
  const reasoningEffort = isReasoningEffort(effort) ? effort : null;
  if (model === null) return { model: null, reasoningEffort };
  if (!modelNames.includes(model)) return EMPTY_COMPOSER_PICK;
  return { model, reasoningEffort };
}

/**
 * 本会话显式选过的模型此刻还作不作数。
 *
 * 与继承缓存同一条判据：名称必须仍在当前配置的可选清单里。停用供应商或删掉
 * `[[models]]` 之后，继续把这一条随轮下发只会让运行侧路由不到，因此这一格
 * 退回配置默认值。`selectableModels` 为 null 表示配置事实还没读回来，
 * 此时保留显式选择，不拿「还没读到」当成「已经删了」。
 */
export function explicitModelInEffect(
  selected: string | null,
  selectableModels: readonly string[] | null,
): string | null {
  const model = clean(selected);
  if (model === null) return null;
  if (selectableModels === null) return model;
  return selectableModels.includes(model) ? model : null;
}
