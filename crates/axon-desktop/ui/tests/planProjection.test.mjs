import test from 'node:test';
import assert from 'node:assert/strict';
import { applyPlan, planHeading } from '../src/conversation/transcript/cells.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { emptyTurn } from '../src/conversation/transcript/turns.ts';

/*
 * 计划投影是输入坞上方固定计划坞与指标条共用的唯一事实源。
 *
 * 这里固定它的对外形状：标题、计数、步骤明细必须同源，且每次更新是覆盖而不是累加；
 * 时间线单元保留的仍是那一刻的快照。渲染层（PlanCard）与指标条都只读这份投影。
 */

/** 带一轮空会话的状态；计划投影写入需要该位置已存在一轮。 */
function stateWithTurn() {
    return { ...INITIAL_CONVERSATION, turns: [emptyTurn('turn-1', 0)] };
}

function plan(partId, steps) {
    return { partId, title: '文档事实源整理', steps };
}

test('计划投影带标题与步骤明细，计数与步骤同源', () => {
    const state = applyPlan(stateWithTurn(), 0, plan('plan:1', [
        { step: '建立基线', status: 'completed' },
        { step: '审计 README', status: 'in_progress' },
        { step: '重构导航', status: 'pending' },
    ]));

    assert.equal(state.plan.title, '文档事实源整理');
    assert.equal(state.plan.completed, 1);
    assert.equal(state.plan.total, 3);
    assert.deepEqual(state.plan.steps.map((item) => item.status),
        ['completed', 'in_progress', 'pending']);

    // 时间线单元与投影读同一份步骤明细：固定坞与正文不会各说一套。
    const cell = state.turns[0].cells.find((item) => item.role === 'plan');
    assert.equal(cell.title, '文档事实源整理');
    assert.deepEqual(cell.steps, state.plan.steps);
});

test('后续更新覆盖投影而不是累加，同一 partId 只留一张时间线单元', () => {
    const first = applyPlan(stateWithTurn(), 0, plan('plan:1', [
        { step: '建立基线', status: 'completed' },
        { step: '审计 README', status: 'completed' },
        { step: '重构导航', status: 'in_progress' },
    ]));
    const second = applyPlan(first, 0, plan('plan:1', [
        { step: '建立基线', status: 'completed' },
    ]));

    assert.equal(second.plan.completed, 1);
    assert.equal(second.plan.total, 1);
    assert.equal(second.turns[0].cells.filter((item) => item.role === 'plan').length, 1);
});

test('未知状态按未完成计入总数，不会被当成已完成', () => {
    const state = applyPlan(stateWithTurn(), 0, plan('plan:1', [
        { step: '已知完成', status: 'completed' },
        { step: '来源未识别', status: 'whatever' },
    ]));

    assert.equal(state.plan.total, 2);
    assert.equal(state.plan.completed, 1);
});

test('位置越界时不写入时间线单元，但仍不破坏既有状态', () => {
    const before = stateWithTurn();
    const after = applyPlan(before, 7, plan('plan:9', [
        { step: '越界步骤', status: 'pending' },
    ]));

    assert.equal(after.turns[0].cells.length, 0);
    assert.equal(after.plan.total, 1);
});

test('默认标题「计划」或空白标题不重复作为副标题渲染，自定义标题原样保留', () => {
    assert.equal(planHeading('计划'), undefined);
    assert.equal(planHeading('  计划  '), undefined);
    assert.equal(planHeading(''), undefined);
    assert.equal(planHeading('   '), undefined);
    assert.equal(planHeading('文档事实源整理'), '文档事实源整理');
});
