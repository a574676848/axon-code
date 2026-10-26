import test from 'node:test';
import assert from 'node:assert/strict';

import { matchBuiltinCommand } from '../src/conversation/builtinCommands.ts';
import {
    mcpCommandReceipt,
    mcpHeaderReading,
    mcpSegmentText,
    mcpSegmentTitle,
} from '../src/conversation/mcpReading.ts';
import { headerClusters } from '../src/conversation/metrics.ts';
import { commandLabel } from '../src/components/conversation/useComposerCommandRun.ts';

/*
 * MCP 读数与 `/mcp` 指令的口径：指标条那一段说什么、什么时候告警、回执怎么写。
 * 三处消费同一个翻译层，因此这里钉住的是「同一件事在不同位置读起来一致」。
 */

function runtimeStatus(overrides = {}) {
    return {
        cwd: '/workspace',
        servers: [],
        totalCount: 0,
        loadedCount: 0,
        failedCount: 0,
        checkedAtMs: 0,
        ...overrides,
    };
}

function metricsInput(mcp) {
    return {
        working: false,
        animationTick: 0,
        model: 'test-model',
        cwd: '/workspace',
        reasoningEffort: null,
        usage: null,
        contextWindow: null,
        firstTokenAvgMs: null,
        generationTpsX10: null,
        elapsedMs: null,
        plan: null,
        lspActiveCount: 0,
        mcp,
    };
}

test('/mcp 只认整条正文是一条内置指令', () => {
    assert.equal(matchBuiltinCommand('/mcp')?.name, 'mcp');
    assert.equal(matchBuiltinCommand('  /mcp  ')?.name, 'mcp', '尾部空白不算参数');
    assert.equal(matchBuiltinCommand('/mcp 看看这个'), null, '带参数的是给模型的话');
    assert.equal(matchBuiltinCommand('看/mcp'), null, '斜杠不在词边界上不是指令');
    assert.equal(matchBuiltinCommand('/nope'), null);
});

test('没有 MCP 时不占指标条，有 MCP 时报台数', () => {
    assert.equal(mcpHeaderReading(null), null);
    assert.equal(mcpHeaderReading(runtimeStatus()), null, '零台且无错误时不出段');

    const reading = mcpHeaderReading(runtimeStatus({ totalCount: 3, loadedCount: 3 }));
    assert.equal(mcpSegmentText(reading), 'MCP 3');
    assert.equal(mcpSegmentTitle(reading), '共 3 台 MCP，全部加载正常');
});

test('有一台没加载起来就用危险色，并把异常台数写在段里', () => {
    const reading = mcpHeaderReading(
        runtimeStatus({ totalCount: 3, loadedCount: 2, failedCount: 1 }),
    );
    assert.equal(mcpSegmentText(reading), 'MCP 2 · 1 异常');

    const clusters = headerClusters(metricsInput(reading));
    const aux = clusters.at(-1);
    assert.deepEqual(
        aux.segments.map((segment) => segment.kind),
        ['mcp'],
        'MCP 属于辅助簇',
    );
    assert.equal(aux.segments[0].tone, 'critical');
});

test('读不到结论时也要画出来，不悄悄消失', () => {
    const reading = mcpHeaderReading(null, '网关不可达');
    assert.notEqual(reading, null);
    assert.equal(mcpSegmentText(reading), 'MCP 异常');
    assert.match(mcpSegmentTitle(reading), /读不到 MCP 运行态/);

    const clusters = headerClusters(metricsInput(reading));
    assert.equal(clusters.at(-1).segments[0].tone, 'critical');
});

test('指标条里 MCP 与 LSP、计划同簇，且排在 LSP 之后', () => {
    const reading = mcpHeaderReading(runtimeStatus({ totalCount: 2, loadedCount: 2 }));
    const clusters = headerClusters({
        ...metricsInput(reading),
        lspActiveCount: 1,
        plan: { completed: 1, total: 2 },
    });
    assert.deepEqual(
        clusters.at(-1).segments.map((segment) => segment.kind),
        ['lsp', 'mcp', 'plan'],
    );
});

test('重载回执说清这一次刷新之后的结论，并带上可展开的服务器清单', () => {
    const ok = mcpCommandReceipt(runtimeStatus({ totalCount: 1, loadedCount: 1 }));
    assert.deepEqual(
        { ok: ok.ok, message: ok.message, servers: ok.servers },
        {
            ok: true,
            message: '已重载 MCP：1 台全部正常',
            servers: [],
        },
    );

    const servers = [
        {
            serverId: 'codebase-memory-mcp',
            toolCount: 12,
            tools: ['search_graph', 'trace_path'],
            failed: false,
            failure: null,
            disposition: 'hit',
        },
        {
            serverId: 'sm-cloud',
            toolCount: 0,
            tools: [],
            failed: true,
            failure: '401 Unauthorized',
            disposition: 'miss',
        },
    ];
    const partial = mcpCommandReceipt({
        ...runtimeStatus({ totalCount: 2, loadedCount: 1, failedCount: 1 }),
        servers,
    });
    assert.deepEqual(partial, {
        ok: false,
        message: '已重载 MCP：1 台正常，1 台异常',
        servers,
    });
    assert.equal(partial.servers[0].tools.length, 2, '成功项要带工具名单');
    assert.equal(partial.servers[1].failure, '401 Unauthorized', '失败项要带原因');

    assert.deepEqual(mcpCommandReceipt(runtimeStatus()), {
        ok: true,
        message: '已重载 MCP：这个工作区没有配置 MCP 服务器',
        servers: [],
    });
    assert.deepEqual(mcpCommandReceipt(runtimeStatus({ error: '探测失败' })), {
        ok: false,
        message: '重载 MCP 失败：探测失败',
    });
});

test('指令执行态有明确的中文进行中文案', () => {
    assert.equal(commandLabel('mcp'), '正在重载 MCP 服务器…');
    assert.equal(
        commandLabel('whatever'),
        '正在执行 /whatever…',
        '未知指令也要有一句人话，不能空着',
    );
});
