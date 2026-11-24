import test from 'node:test';
import assert from 'node:assert/strict';
import { mergeDiscoveredModels } from '../src/modelCatalog.ts';

/** 模型草稿的公共字段，测试里只关心 name/id/归属。 */
const base = {
  context_window: 1000000,
  max_output_tokens: 8192,
  supports_vision: true,
  supports_embedding: false,
  embedding_dimension: 0,
  requires_reasoning_content_on_assistant_messages: false,
};

test('同一上游 ID 配多个路由名时，采用新候选不得吞掉另一个别名', () => {
  const models = [
    { ...base, name: 'fast_model', id: 'deepseek-v4.1-flash-1017', provider: 'sm-wenwen-chat' },
    {
      ...base,
      name: 'deepseek-v4.1-flash',
      id: 'deepseek-v4.1-flash-1017',
      provider: 'sm-wenwen-chat',
      requires_reasoning_content_on_assistant_messages: true,
    },
  ];

  const next = mergeDiscoveredModels(
    models,
    [{ id: 'gpt-6.1-sol', name: 'gpt-6-astra' }],
    'sm-wenwen-chat',
  );

  assert.deepEqual(
    next.map((model) => model.name),
    ['fast_model', 'deepseek-v4.1-flash', 'gpt-6-astra'],
    '既有的两条同 ID 别名都必须原样保留，新候选追加在末尾',
  );
  assert.equal(next[1].requires_reasoning_content_on_assistant_messages, true);
});

test('同名候选视为已存在，不重复追加且不覆盖用户校正过的参数', () => {
  const models = [
    {
      ...base,
      name: 'gpt-6-astra',
      id: 'gpt-6.1-sol',
      provider: 'sm-wenwen-chat',
      context_window: 800000,
    },
  ];

  const next = mergeDiscoveredModels(
    models,
    [{ id: 'gpt-6.1-sol', name: 'gpt-6-astra', context_window: 128000 }],
    'sm-wenwen-chat',
  );

  assert.equal(next.length, 1);
  assert.equal(next[0].context_window, 800000, '已配置的容量参数不能被上游值覆盖');
});

test('候选缺少 name 时用 id 兜底，空名候选直接跳过', () => {
  const next = mergeDiscoveredModels(
    [],
    [{ id: 'no-name-model' }, { id: '  ', name: '   ' }],
    'sm-wenwen-chat',
  );

  assert.deepEqual(
    next.map((model) => model.name),
    ['no-name-model'],
  );
  assert.equal(next[0].id, 'no-name-model');
  assert.equal(next[0].provider, 'sm-wenwen-chat');
});
