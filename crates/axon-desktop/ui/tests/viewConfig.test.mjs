import test from 'node:test';
import assert from 'node:assert/strict';

import { KEY_DEFAULT_MODEL, modelLabelFor, resolveConfig } from '../src/conversation/viewConfig.ts';

/*
 * 配置投影的口径：输入坞读展示名，上下文窗口与展示名按目录映射。
 * 路由缺失时如何回落也在这里钉住。
 */

const MODEL = {
    name: 'deepseek-v4.1-flash',
    id: 'deepseek-v4.1-flash-1017',
    provider: 'sm-wenwen-chat',
    context_window: 262_144,
    supports_vision: false,
    supports_embedding: false,
};

function configOf(modelName, models = [MODEL]) {
    return { flat_values: { [KEY_DEFAULT_MODEL]: modelName }, models };
}

test('配置投影只给展示名与上下文窗口，不推导供应商路由', () => {
    const resolved = resolveConfig(configOf('deepseek-v4.1-flash'));
    assert.equal(resolved.model, 'deepseek-v4.1-flash', '输入坞仍然读展示名');
    assert.equal(resolved.contextWindow, 262_144);
    assert.equal(
        Object.hasOwn(resolved, 'route'),
        false,
        '渠道由运行时按池选出，配置投影不能自称知道本轮走哪一家',
    );
});

test('配置里认不出这个模型时窗口给空，展示名原样保留', () => {
    const resolved = resolveConfig(configOf('someone-else-model'));
    assert.equal(resolved.model, 'someone-else-model');
    assert.equal(resolved.contextWindow, null);
});

test('固定模型（cron 车道）同样按它自己的展示名取名', () => {
    const resolved = resolveConfig(configOf('deepseek-v4.1-flash'), 'deepseek-v4.1-flash');
    assert.equal(resolved.model, 'deepseek-v4.1-flash');
    assert.equal(resolved.contextWindow, 262_144);
});

test('真实运行面按同 provider 同 id 精确映射展示名', () => {
    const catalog = resolveConfig(configOf('deepseek-v4.1-flash')).catalog;
    assert.equal(
        modelLabelFor(catalog, {
            provider: 'sm-wenwen-chat',
            model: 'deepseek-v4.1-flash-1017',
        }),
        'deepseek-v4.1-flash',
    );
});

test('provider 改名或迁移后按 upstream id 兜底映射', () => {
    const catalog = resolveConfig(configOf('deepseek-v4.1-flash')).catalog;
    assert.equal(
        modelLabelFor(catalog, {
            provider: 'removed-provider',
            model: 'deepseek-v4.1-flash-1017',
        }),
        'deepseek-v4.1-flash',
    );
});

test('会话模型已从配置移除时不显示它，交给调用方回退配置默认值', () => {
    const catalog = resolveConfig(configOf('deepseek-v4.1-flash')).catalog;
    assert.equal(
        modelLabelFor(catalog, {
            provider: 'removed-provider',
            model: 'removed-model-id',
        }),
        null,
    );
    assert.equal(modelLabelFor(catalog, null), null);
});
