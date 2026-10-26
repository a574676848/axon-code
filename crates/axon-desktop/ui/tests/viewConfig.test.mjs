import test from 'node:test';
import assert from 'node:assert/strict';

import { KEY_DEFAULT_MODEL, resolveConfig } from '../src/conversation/viewConfig.ts';
import { routeFromActivity } from '../src/conversation/useTurnModelRoute.ts';

/*
 * 配置投影的口径：输入坞读展示名，工作状态行读完整路由（供应商/模型 id）。
 * 两者都由这里算出，因此路由缺失时如何回落也在这里钉住。
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

test('配置里的模型能解析时就给出供应商/模型 id 的完整路由', () => {
    const resolved = resolveConfig(configOf('deepseek-v4.1-flash'));
    assert.equal(resolved.model, 'deepseek-v4.1-flash', '输入坞仍然读展示名');
    assert.equal(resolved.route, 'sm-wenwen-chat/deepseek-v4.1-flash-1017');
    assert.equal(resolved.contextWindow, 262_144);
});

test('配置里认不出这个模型时路由回落到原值，不编造供应商', () => {
    const resolved = resolveConfig(configOf('someone-else-model'));
    assert.equal(resolved.route, 'someone-else-model');
    assert.equal(resolved.contextWindow, null);
});

test('固定模型（cron 车道）同样按它自己的路由取名', () => {
    const resolved = resolveConfig(configOf('deepseek-v4.1-flash'), 'deepseek-v4.1-flash');
    assert.equal(resolved.model, 'deepseek-v4.1-flash');
    assert.equal(resolved.route, 'sm-wenwen-chat/deepseek-v4.1-flash-1017');
});

test('配置里已经写成供应商/模型时原样沿用，不再被目录里的同名条目覆盖', () => {
    const resolved = resolveConfig(configOf('sm-wenwen-chat/deepseek-v4.1-flash-1017'));
    assert.equal(resolved.model, 'sm-wenwen-chat/deepseek-v4.1-flash-1017');
    assert.equal(
        resolved.route,
        'sm-wenwen-chat/deepseek-v4.1-flash-1017',
        '用户明确指定的供应商不能被换成目录里的第一条同名模型',
    );
});

test('状态事件的第一段是路由时取出来，否则不凭空猜', () => {
    assert.equal(
        routeFromActivity('sm-wenwen-chat/deepseek-v4.1-flash-1017 正在连接'),
        'sm-wenwen-chat/deepseek-v4.1-flash-1017',
    );
    assert.equal(routeFromActivity('生成回复中'), null);
    assert.equal(routeFromActivity(null), null);
});
