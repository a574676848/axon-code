import test from 'node:test';
import assert from 'node:assert/strict';

import { KEY_DEFAULT_MODEL, modelLabelFor, resolveConfig, resolveEffectiveModel } from '../src/conversation/viewConfig.ts';

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

const ROUTED = {
    name: 'gpt-6-astra',
    id: 'gpt-6-astra',
    provider: 'codex-onetoken',
    context_window: 1_000_000,
};

test('生效模型与最大上下文必须同源：会话模型优先，窗口取它自己的口径', () => {
    const resolved = resolveConfig(configOf('deepseek-v4.1-flash', [MODEL, ROUTED]));
    const effective = resolveEffectiveModel(resolved, null, null, {
        provider: 'codex-onetoken',
        model: 'gpt-6-astra',
    });
    assert.equal(effective.label, 'gpt-6-astra');
    assert.equal(effective.contextWindow, 1_000_000, '窗口要跟着会话模型，不能回落到默认模型');
});

test('没有会话事实时，生效模型与窗口都取配置默认值', () => {
    const resolved = resolveConfig(configOf('deepseek-v4.1-flash', [MODEL, ROUTED]));
    const effective = resolveEffectiveModel(resolved, null, null, null);
    assert.equal(effective.label, 'deepseek-v4.1-flash');
    assert.equal(effective.contextWindow, 262_144);
});

/* 同一 upstream 模型在配置里挂两个别名：只按 provider 与 id 取第一条会显示错模型。 */
const DUAL_ALIAS = [
    { ...MODEL, name: 'fast_model', context_window: 131_072 },
    { ...MODEL, name: 'deepseek-v4.1-flash', context_window: 262_144 },
];

test('同一 provider 与 upstream id 有多条别名时，按记录里的别名还原展示名', () => {
    const config = resolveConfig(configOf('fast_model', DUAL_ALIAS));
    const lane = {
        provider: 'sm-wenwen-chat',
        model: 'deepseek-v4.1-flash-1017',
        name: 'deepseek-v4.1-flash',
    };
    assert.equal(modelLabelFor(config.catalog, lane), 'deepseek-v4.1-flash');
    const effective = resolveEffectiveModel(config, null, null, lane);
    assert.equal(effective.label, 'deepseek-v4.1-flash');
    assert.equal(effective.contextWindow, 262_144, '窗口必须跟着别名走，不能取同 id 的另一条');
});

test('记录里的别名对不上 provider 或 id 时，仍退回 id 匹配', () => {
    const config = resolveConfig(configOf('fast_model', DUAL_ALIAS));
    assert.equal(
        modelLabelFor(config.catalog, {
            provider: 'sm-wenwen-chat',
            model: 'deepseek-v4.1-flash-1017',
            name: 'removed-alias',
        }),
        'fast_model',
        '别名已从配置移除，只能按 provider 与 id 取第一条',
    );
    assert.equal(
        modelLabelFor(config.catalog, {
            provider: 'another-provider',
            model: 'removed-model-id',
            name: 'deepseek-v4.1-flash',
        }),
        null,
        '别名与 id 都对不上时交给调用方回退配置默认模型',
    );
});

test('渠道改名后，别名与 upstream id 同时对上仍然作数', () => {
    const config = resolveConfig(configOf('fast_model', DUAL_ALIAS));
    assert.equal(
        modelLabelFor(config.catalog, {
            provider: 'renamed-provider',
            model: 'deepseek-v4.1-flash-1017',
            name: 'deepseek-v4.1-flash',
        }),
        'deepseek-v4.1-flash',
        'provider 变了但别名与 id 都指向同一条，不能退回同 id 的第一条',
    );
});
