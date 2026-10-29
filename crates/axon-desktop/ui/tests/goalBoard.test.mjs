import test from 'node:test';
import assert from 'node:assert/strict';

import {
    COLUMN_CARD_LIMIT,
    GOAL_COLUMNS,
    INITIAL_CARD_LIMIT,
    boardColumns,
    cardActionLabel,
    columnHeaderDot,
    countGoals,
    countSummary,
    distinctObjectivePreview,
    executionStatusLabel,
    goalContractSummary,
    goalLocatorText,
    goalProgressCue,
    goalStatusMeta,
    hasSessionBinding,
    hasUnsettled,
    matchesFilter,
    matchesQuery,
    isSettled,
    selectGoals,
} from '../src/goals/model.ts';

/** 造一张卡片：默认是绑定了执行会话的工作区目标。 */
function card(overrides) {
    return {
        goalId: 'goal-1',
        title: '修复登录回归',
        status: 'active',
        revision: 1,
        objectivePreview: '恢复登录链路的全部失败用例',
        stepCount: 3,
        nextStep: '定位失败用例',
        waitingReason: null,
        priority: 0,
        deadlineMs: null,
        scopeKind: 'workspace',
        workspaceId: 'workspace-1',
        workspaceName: 'axon-code',
        workspacePath: '/repo',
        createdAtMs: 1_000,
        updatedAtMs: 2_000,
        lastActivityAtMs: 3_000,
        latestExecution: {
            executionId: 'execution-1',
            status: 'running',
            attempt: 1,
            sessionId: 'goal-session-goal-1',
            createdAtMs: 1_000,
            updatedAtMs: 2_000,
        },
        latestResult: null,
        ...overrides,
    };
}

test('状态映射覆盖七态，且文案只有中文', () => {
    const expected = [
        ['proposed', '待开始', 'idle', 'neutral', 'proposed'],
        ['active', '进行中', 'ongoing', 'accent', 'running'],
        ['waiting', '等待中', 'warning', 'warning', 'running'],
        ['paused', '已暂停', 'warning', 'warning', 'paused'],
        ['completed', '已完成', 'done', 'ok', 'settled'],
        ['failed', '已失败', 'error', 'danger', 'settled'],
        ['cancelled', '已取消', 'idle', 'neutral', 'settled'],
    ];
    for (const [status, label, dot, chipTone, column] of expected) {
        const meta = goalStatusMeta(status);
        assert.equal(meta.label, label, status);
        assert.equal(meta.dot, dot, status);
        assert.equal(meta.chipTone, chipTone, status);
        assert.equal(meta.column, column, status);
        // 文案里不得出现后端枚举。
        assert.ok(!/[a-z]/i.test(meta.label), `${status} 的文案混进了枚举`);
    }
});

test('执行状态也给中文，界面不出现后端枚举', () => {
    assert.equal(executionStatusLabel('queued'), '排队中');
    assert.equal(executionStatusLabel('running'), '进行中');
    assert.equal(executionStatusLabel('uncertain'), '结果不确定');
    assert.equal(executionStatusLabel('completed'), '已完成');
    // 未知取值原样回落，不伪装成某个已知状态。
    assert.equal(executionStatusLabel('brand_new'), 'brand_new');
});

test('未知状态回落为待开始，不抛错也不显示空文案', () => {
    assert.equal(goalStatusMeta('nonsense').label, '待开始');
});

test('已收口判定只认三个终态', () => {
    assert.equal(isSettled('completed'), true);
    assert.equal(isSettled('failed'), true);
    assert.equal(isSettled('cancelled'), true);
    assert.equal(isSettled('waiting'), false);
    assert.equal(isSettled('paused'), false);
});

test('四栏顺序固定，初始挂载上限等于四栏各二十张', () => {
    assert.deepEqual(
        GOAL_COLUMNS.map((column) => column.id),
        ['proposed', 'running', 'paused', 'settled'],
    );
    assert.equal(INITIAL_CARD_LIMIT, 80);
    assert.equal(COLUMN_CARD_LIMIT, 20);
});

test('按状态归组，等待中与进行中同栏', () => {
    const goals = [
        card({ goalId: 'a', status: 'proposed' }),
        card({ goalId: 'b', status: 'active' }),
        card({ goalId: 'c', status: 'waiting' }),
        card({ goalId: 'd', status: 'paused' }),
        card({ goalId: 'e', status: 'completed' }),
        card({ goalId: 'f', status: 'failed' }),
        card({ goalId: 'g', status: 'cancelled' }),
    ];
    const columns = boardColumns(goals, []);
    assert.deepEqual(
        columns.map((column) => column.total),
        [1, 2, 1, 3],
    );
    assert.deepEqual(
        columns[1].visible.map((goal) => goal.goalId),
        ['b', 'c'],
    );
});

test('单栏超过二十张时折叠并给出其余张数，展开后不再限制', () => {
    const goals = Array.from({ length: 25 }, (_, index) =>
        card({ goalId: `goal-${index}`, status: 'active' }),
    );
    const collapsed = boardColumns(goals, []);
    const running = collapsed.find((column) => column.id === 'running');
    assert.equal(running.visible.length, 20);
    assert.equal(running.hidden, 5);
    assert.equal(running.total, 25);

    const expanded = boardColumns(goals, ['running']);
    const widened = expanded.find((column) => column.id === 'running');
    assert.equal(widened.visible.length, 25);
    assert.equal(widened.hidden, 0);
});

test('没有成员的栏目给出空切片，不折叠也不留占位', () => {
    const goals = [card({ goalId: 'only', status: 'active' })];
    const columns = boardColumns(goals, []);
    const empty = columns.filter((column) => column.id !== 'running');
    assert.equal(empty.length, 3);
    for (const column of empty) {
        assert.equal(column.total, 0, column.id);
        assert.deepEqual(column.visible, [], column.id);
        assert.equal(column.hidden, 0, column.id);
    }
});

test('搜索覆盖标题、预览、工作区与下一步，且大小写不敏感', () => {
    const goal = card({ title: 'Fix Login', workspaceName: 'Axon-Code' });
    assert.equal(matchesQuery(goal, ''), true);
    assert.equal(matchesQuery(goal, '   '), true);
    assert.equal(matchesQuery(goal, 'login'), true);
    assert.equal(matchesQuery(goal, 'axon'), true);
    assert.equal(matchesQuery(goal, '定位失败'), true);
    assert.equal(matchesQuery(goal, '不存在的词'), false);
});

test('状态分段与搜索串叠加生效', () => {
    const goals = [
        card({ goalId: 'open-1', status: 'active' }),
        card({ goalId: 'open-2', status: 'waiting' }),
        card({ goalId: 'done-1', status: 'completed' }),
    ];
    assert.equal(selectGoals(goals, '', 'all').length, 3);
    assert.equal(selectGoals(goals, '', 'open').length, 2);
    assert.deepEqual(
        selectGoals(goals, '', 'settled').map((goal) => goal.goalId),
        ['done-1'],
    );
    assert.equal(selectGoals(goals, '修复登录', 'settled').length, 1);
    assert.equal(selectGoals(goals, '不存在的词', 'all').length, 0);
});

test('计数口径取回执的 returned，分段计数来自同一批目标', () => {
    const goals = [
        card({ goalId: 'a', status: 'active' }),
        card({ goalId: 'b', status: 'paused' }),
        card({ goalId: 'c', status: 'completed' }),
    ];
    assert.deepEqual(countGoals(goals, 3), { total: 3, open: 2, settled: 1 });
    // total 是服务端回执口径：调用方只拿到部分数组时，总数仍以 returned 为准。
    assert.equal(countGoals(goals.slice(0, 2), 3).total, 3);
});

test('截断时计数旁写明只显示 N 个，且不写方向与非产品态术语', () => {
    const counts = { total: 3, open: 2, settled: 1 };
    assert.equal(
        countSummary(counts, false, 256),
        '共 3 个目标 · 未完成 2 · 已结束 1',
    );
    const truncated = countSummary(counts, true, 256);
    assert.equal(truncated, '共 3 个目标 · 未完成 2 · 已结束 1 · 已达上限，仅显示 256 个');
    // 服务端按更新时间升序出页，读满上限拿到的是最早一批，「最近」会是错话；也不出现「收口」等内部黑话。
    assert.ok(!truncated.includes('最近'));
    assert.ok(!truncated.includes('收口'));
});

test('等待原因优先于下一步，都没有时定位行为空', () => {
    assert.equal(goalLocatorText(card({ waitingReason: '需要确认口径' })), '等待 · 需要确认口径');
    assert.equal(goalLocatorText(card()), '下一步 · 定位失败用例');
    assert.equal(goalLocatorText(card({ nextStep: null })), '');

    assert.deepEqual(goalProgressCue(card({ waitingReason: '需要确认口径' })), {
        kind: 'waiting',
        label: '等待',
        text: '需要确认口径',
        tone: 'warning',
    });
    assert.deepEqual(goalProgressCue(card()), {
        kind: 'next',
        label: '下一步',
        text: '定位失败用例',
        tone: 'default',
    });
    assert.equal(goalProgressCue(card({ nextStep: null })), null);
});

test('目标摘要与标题完全相同时去重，不同时保留预览', () => {
    assert.equal(
        distinctObjectivePreview(card({ title: '目标看板验收', objectivePreview: '  目标看板验收  ' })),
        '',
    );
    assert.equal(
        distinctObjectivePreview(card({ title: '目标看板验收', objectivePreview: '把目标看板的数据态验收跑通' })),
        '把目标看板的数据态验收跑通',
    );
});

test('栏目头状态点按栏内真实成员状态投影，空栏保持静息', () => {
    assert.equal(columnHeaderDot('running', []), 'idle');
    assert.equal(columnHeaderDot('running', [card({ status: 'waiting' })]), 'warning');
    assert.equal(
        columnHeaderDot('running', [card({ status: 'waiting' }), card({ status: 'active' })]),
        'ongoing',
    );
    assert.equal(columnHeaderDot('settled', [card({ status: 'failed' })]), 'error');
    assert.equal(
        columnHeaderDot('settled', [card({ status: 'failed' }), card({ status: 'completed' })]),
        'done',
    );
});

test('目标概览条统计文案按步骤与验收数量组合', () => {
    assert.equal(goalContractSummary(2, 1), '2 步 · 1 项验收');
    assert.equal(goalContractSummary(3, 0), '3 步');
    assert.equal(goalContractSummary(0, 2), '2 项验收');
    assert.equal(goalContractSummary(0, 0), '目标说明');
});

test('只有同时有执行会话与工作区才进入会话，否则退回详情', () => {
    assert.equal(hasSessionBinding(card()), true);
    assert.equal(cardActionLabel(card()), '进入会话');

    const noExecution = card({ latestExecution: null });
    assert.equal(hasSessionBinding(noExecution), false);
    assert.equal(cardActionLabel(noExecution), '查看详情');

    const noSession = card({
        latestExecution: { ...card().latestExecution, sessionId: null },
    });
    assert.equal(hasSessionBinding(noSession), false);

    // 全局目标解析不到工作区：字段为空，因此降级为详情而不是死点击。
    const globalGoal = card({ workspaceId: null, workspaceName: null, workspacePath: null });
    assert.equal(hasSessionBinding(globalGoal), false);
    assert.equal(cardActionLabel(globalGoal), '查看详情');
});

test('全部收口即停止轮询', () => {
    assert.equal(hasUnsettled([card({ status: 'completed' })]), false);
    assert.equal(hasUnsettled([card({ status: 'completed' }), card({ status: 'waiting' })]), true);
    assert.equal(hasUnsettled([]), false);
});
