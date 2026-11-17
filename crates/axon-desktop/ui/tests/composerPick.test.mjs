import test from 'node:test';
import assert from 'node:assert/strict';

import {
    EMPTY_COMPOSER_PICK,
    inheritedComposerPick,
} from '../src/conversation/composerPick.ts';

/*
 * 新会话继承输入坞选择的口径：缓存记的是模型名称，名称必须仍在本配置的模型目录里；
 * 名称消失时整份作废，回到 agent.default_model 与 agent.reasoning_effort。
 */

const NAMES = ['deepseek-v4.1-flash', 'glm-5'];

test('缓存里的模型名称仍在配置里时，模型与强度一起继承', () => {
    assert.deepEqual(
        inheritedComposerPick(NAMES, { model: 'glm-5', reasoningEffort: 'xhigh' }),
        { model: 'glm-5', reasoningEffort: 'xhigh' },
    );
});

test('模型名称已从配置里消失时整份作废，模型与强度都回到配置默认值', () => {
    assert.deepEqual(
        inheritedComposerPick(NAMES, { model: '已下线的模型', reasoningEffort: 'high' }),
        EMPTY_COMPOSER_PICK,
    );
    // 配置还没读回来（目录为空）时同样不能凭空继承一个名称。
    assert.deepEqual(
        inheritedComposerPick([], { model: 'glm-5', reasoningEffort: 'high' }),
        EMPTY_COMPOSER_PICK,
    );
});

test('只选过强度时模型仍由配置默认值决定，强度照旧沿用', () => {
    assert.deepEqual(
        inheritedComposerPick(NAMES, { model: null, reasoningEffort: 'low' }),
        { model: null, reasoningEffort: 'low' },
    );
});

test('缓存记的是名称而不是 upstream id：只写 id 不算命中', () => {
    // deepseek-v4.1-flash-1017 是该模型的 id，配置目录给的是名称。
    assert.deepEqual(
        inheritedComposerPick(NAMES, {
            model: 'deepseek-v4.1-flash-1017',
            reasoningEffort: 'high',
        }),
        EMPTY_COMPOSER_PICK,
    );
});

test('空白缓存与手改出来的非法档位都不参与继承', () => {
    assert.deepEqual(
        inheritedComposerPick(NAMES, { model: '  ', reasoningEffort: '' }),
        EMPTY_COMPOSER_PICK,
    );
    assert.deepEqual(
        inheritedComposerPick(NAMES, { model: 'glm-5', reasoningEffort: 'turbo' }),
        { model: 'glm-5', reasoningEffort: null },
    );
    assert.deepEqual(
        inheritedComposerPick(NAMES, { model: ' glm-5 ', reasoningEffort: ' high ' }),
        { model: 'glm-5', reasoningEffort: 'high' },
    );
});
