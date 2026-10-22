import test from 'node:test';
import assert from 'node:assert/strict';
import { sessionGenerationMetrics } from '../src/conversation/sessionGeneration.ts';

/** 一轮的最小事实：首字、解码窗口、输出与推理 token。 */
const turn = (firstTokenAvgMs, decodeMs, outputTokens, reasoningOutputTokens = 0) => ({
    firstTokenAvgMs,
    decodeMs,
    usage: { outputTokens, reasoningOutputTokens },
});

/** 带 Provider 实测读数的一轮：用量累计可能被重复帧放大，实测值才是分子。 */
const measuredTurn = (decodeMs, outputTokens, reasoningOutputTokens, usageOutput, usageReasoning) => ({
    firstTokenAvgMs: null,
    decodeMs,
    usage: { outputTokens: usageOutput, reasoningOutputTokens: usageReasoning },
    generationOutputTokens: outputTokens,
    generationReasoningTokens: reasoningOutputTokens,
});

test('首字取各轮算术平均，慢轮与快轮同等权重', () => {
    const metrics = sessionGenerationMetrics([turn(2_000, 8_000, 100), turn(6_000, 8_000, 300)]);
    assert.equal(metrics.firstTokenAvgMs, 4_000);
    assert.equal(metrics.ttftSamples, 2);
});

test('速率按解码窗口加权，Σ(输出+推理)/Σ窗口 且含思考输出', () => {
    // (100 + 20) + (300 + 80) = 500 token；窗口 8s + 2s = 10s → 50.0 tok/s
    const metrics = sessionGenerationMetrics([
        turn(2_000, 8_000, 100, 20),
        turn(6_000, 2_000, 300, 80),
    ]);
    assert.equal(metrics.generationTpsX10, 500);
    assert.equal(metrics.decodeSamples, 2);
});

test('在跑轮的首字即时参与，但还没收口就不进速率分母', () => {
    const metrics = sessionGenerationMetrics([turn(4_000, 10_000, 200), turn(2_000, null, 40)]);
    assert.equal(metrics.firstTokenAvgMs, 3_000, '在跑轮的首字要算进去');
    assert.equal(metrics.decodeSamples, 1, '在跑轮还没有解码窗口');
    assert.equal(metrics.generationTpsX10, 200, '4s 解码 200 token = 50.0 tok/s');
});

test('没有任何样本时不返回 0，也不编造读数', () => {
    assert.deepEqual(sessionGenerationMetrics([]), {
        firstTokenAvgMs: null,
        generationTpsX10: null,
        ttftSamples: 0,
        decodeSamples: 0,
    });
    const noTokens = sessionGenerationMetrics([turn(null, 5_000, 0)]);
    assert.equal(noTokens.firstTokenAvgMs, null);
    assert.equal(noTokens.generationTpsX10, null);
});

test('分子优先用 Provider 实测输出，重复用量帧不会把速率翻倍', () => {
    // 实测：20s 窗口 500 token → 25.0 tok/s；用量累计被重复帧放大到 1000 也不参与。
    const metrics = sessionGenerationMetrics([measuredTurn(20_000, 400, 100, 800, 200)]);
    assert.equal(metrics.generationTpsX10, 250);
});

test('没有实测读数的历史轮继续用用量累计兜底', () => {
    const metrics = sessionGenerationMetrics([turn(null, 10_000, 100, 20)]);
    assert.equal(metrics.generationTpsX10, 120, '10s 解码 120 token = 12.0 tok/s');
});
