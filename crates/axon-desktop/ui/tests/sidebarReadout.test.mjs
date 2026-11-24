import test from 'node:test';
import assert from 'node:assert/strict';
import { sidebarReadouts } from '../src/conversation/sidebarReading.ts';

/** 一台正常、一台异常的 MCP 运行态。 */
function mcp(overrides = {}) {
    return { totalCount: 3, loadedCount: 2, failedCount: 1, error: null, ...overrides };
}

test('档案名之后先报 MCP 台数，再报技能条数', () => {
    const readouts = sidebarReadouts(mcp(), 12);

    assert.deepEqual(
        readouts.map((item) => item.kind),
        ['mcp', 'skill'],
        '侧栏连接行读作「档案 MCP 技能 延迟」',
    );
    assert.equal(readouts[0].text, 'MCP 2 · 1 异常');
    assert.equal(readouts[0].tone, 'critical', '有一台没加载起来时整段取危险色');
    assert.equal(readouts[1].text, '技能 12');
    assert.equal(readouts[1].title, '共 12 项已安装技能');
    assert.equal(readouts[1].tone, 'default');
});

test('全部正常时只报台数，并给出完整的悬停说明', () => {
    const readouts = sidebarReadouts(
        { totalCount: 3, loadedCount: 3, failedCount: 0, error: null },
        32,
    );

    assert.equal(readouts[0].text, 'MCP 3');
    assert.equal(readouts[0].title, '共 3 台 MCP，全部加载正常');
    assert.equal(readouts[0].tone, 'default');
});

test('读不到 MCP 结论时必须说出来，不静默消失', () => {
    const readouts = sidebarReadouts({ totalCount: 0, loadedCount: 0, failedCount: 0, error: '网关不可达' }, null);

    assert.equal(readouts.length, 1, '技能清单还没就绪时不占位，MCP 异常照常上报');
    assert.equal(readouts[0].text, 'MCP 异常');
    assert.equal(readouts[0].tone, 'critical');
    assert.equal(readouts[0].title, '读不到 MCP 运行态：网关不可达');
});

test('没有 MCP 与零条技能时整段不占位', () => {
    for (const skillCount of [null, undefined, 0]) {
        assert.deepEqual(
            sidebarReadouts(null, skillCount),
            [],
            `skillCount=${String(skillCount)} 时侧栏不应出现任何辅助读数`,
        );
    }
    assert.deepEqual(sidebarReadouts(undefined, 5).map((item) => item.kind), ['skill']);
});
