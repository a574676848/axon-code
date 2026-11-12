import test from 'node:test';
import assert from 'node:assert/strict';

import { hasUsageObservation, lastObservedUsage } from '../src/conversation/sessionUsage.ts';

const EMPTY = {
    inputTokens: 0,
    cacheReadInputTokens: 0,
    cacheWriteInputTokens: 0,
    outputTokens: 0,
    reasoningOutputTokens: 0,
    currentContextTokens: 0,
    systemPromptTokens: 0,
    toolDefinitionTokens: 0,
    conversationTokens: 0,
};

function usage(overrides = {}) {
    return { ...EMPTY, ...overrides };
}

test('新一轮还没有用量时沿用上一轮，指标条不清零', () => {
    const observed = usage({ inputTokens: 1_000, cacheReadInputTokens: 500, currentContextTokens: 336_000 });

    assert.equal(lastObservedUsage([{ usage: observed }, { usage: EMPTY }]), observed);
});

test('整条会话一次都没观测过时返回 null，交给零读数渲染', () => {
    assert.equal(lastObservedUsage([]), null);
    assert.equal(lastObservedUsage([{ usage: EMPTY }, { usage: EMPTY }]), null);
});

test('最新一轮已有观测就直接用它', () => {
    const older = usage({ inputTokens: 900 });
    const newest = usage({ inputTokens: 1_200 });

    assert.equal(lastObservedUsage([{ usage: older }, { usage: newest }]), newest);
});

test('只有上下文总量也算观测：Provider 可以先报窗口占用再报分桶', () => {
    const contextOnly = usage({ currentContextTokens: 1_024 });

    assert.equal(hasUsageObservation(contextOnly), true);
    assert.equal(hasUsageObservation(EMPTY), false);
    assert.equal(lastObservedUsage([{ usage: EMPTY }, { usage: contextOnly }]), contextOnly);
});
