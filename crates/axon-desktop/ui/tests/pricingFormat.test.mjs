import test from 'node:test';
import assert from 'node:assert/strict';
import {
    filterRules,
    formatAgo,
    formatMultiplier,
    formatPriceCell,
    formatPricePerMillion,
    formatRuleTime,
    normalizePricingStyle,
    parseMultiplier,
    parsePriceInput,
    pricingStyleLabel,
} from '../src/components/settings/pricingFormat.ts';

const rule = (modelPattern, overrides = {}) => ({
    id: `pr_${modelPattern}`,
    modelPattern,
    pricingStyle: 'openai',
    inputPricePerMMicros: 800_000,
    outputPricePerMMicros: 4_000_000,
    cacheReadPricePerMMicros: 80_000,
    cacheWritePricePerMMicros: 0,
    multiplierBasisPoints: 10_000,
    priority: 0,
    createdAtMs: 1,
    updatedAtMs: 1,
    ...overrides,
});

test('单价按美元 / 百万 Token 读写，最多四位小数并去掉尾随零', () => {
    assert.equal(formatPricePerMillion(800_000), '0.8');
    assert.equal(formatPricePerMillion(4_000_000), '4');
    assert.equal(formatPricePerMillion(187_500), '0.1875');
    assert.equal(formatPricePerMillion(0), '0');
    assert.equal(formatPriceCell(800_000), '$0.8');
    assert.equal(formatPriceCell(0), '$0');
});

test('单价解析拒绝负数与超限，空串按 0 处理', () => {
    assert.equal(parsePriceInput('0.8'), 800_000);
    assert.equal(parsePriceInput('0.1875'), 187_500);
    assert.equal(parsePriceInput(''), 0);
    assert.equal(parsePriceInput('  '), 0);
    assert.equal(parsePriceInput('-1'), null);
    assert.equal(parsePriceInput('abc'), null);
    assert.equal(parsePriceInput('9999999999'), null, '超出上限的单价必须被拒绝');
});

test('倍率按 basis points 换算，零与负数被拒绝', () => {
    assert.equal(parseMultiplier('1'), 10_000);
    assert.equal(parseMultiplier('1.15'), 11_500);
    assert.equal(parseMultiplier(''), 10_000, '留空按 1.0 倍');
    assert.equal(parseMultiplier('0'), null);
    assert.equal(parseMultiplier('-2'), null);
    assert.equal(parseMultiplier('99'), null, '超过 10 倍的倍率被拒绝');
    assert.equal(formatMultiplier(10_000), '1');
    assert.equal(formatMultiplier(11_500), '1.15');
});

test('规则时间与多久以前按本地日历读', () => {
    const now = new Date(2026, 9, 21, 16, 30, 0).getTime();
    assert.equal(formatRuleTime(new Date(2026, 9, 21, 9, 5).getTime()), '10/21 09:05');
    assert.equal(formatRuleTime(0), '无记录');
    assert.equal(formatAgo(now - 30_000, now), '刚刚');
    assert.equal(formatAgo(now - 25 * 60_000, now), '25 分钟前');
    assert.equal(formatAgo(now - 5 * 3_600_000, now), '5 小时前');
    assert.equal(formatAgo(now - 39 * 86_400_000, now), '39 天前');
    assert.equal(formatAgo(null, now), '从未运行');
});

test('计费风格有中文展示名', () => {
    assert.equal(pricingStyleLabel('openai'), 'OpenAI 风格');
    assert.equal(pricingStyleLabel('claude'), 'Claude 风格');
});

test('计费风格归一化：网关回传的 open_ai 与手写的 openai 收成同一个取值', () => {
    assert.equal(normalizePricingStyle('open_ai'), 'openai');
    assert.equal(normalizePricingStyle('openai'), 'openai');
    assert.equal(normalizePricingStyle(' OpenAI '), 'openai');
    assert.equal(normalizePricingStyle('claude'), 'claude');
    assert.equal(normalizePricingStyle('CLAUDE'), 'claude');
    // 归一化后的展示名与归一化前一致：读回来的值直接上屏也要对。
    assert.equal(pricingStyleLabel('open_ai'), 'OpenAI 风格');
});

test('规则搜索按匹配式做大小写不敏感的包含匹配', () => {
    const rules = [rule('deepseek-*'), rule('gpt-5'), rule('Claude-3-5-Sonnet')];
    assert.equal(filterRules(rules, '').length, 3);
    assert.deepEqual(
        filterRules(rules, 'DEEP').map((item) => item.modelPattern),
        ['deepseek-*'],
    );
    assert.deepEqual(
        filterRules(rules, 'claude').map((item) => item.modelPattern),
        ['Claude-3-5-Sonnet'],
    );
    assert.equal(filterRules(rules, 'gemini').length, 0);
});
