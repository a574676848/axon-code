import test from 'node:test';
import assert from 'node:assert/strict';
import { applyPlan, planHeading, planProjectionOf } from '../src/conversation/transcript/cells.ts';
import { applyFrames } from '../src/conversation/transcript/frames.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { EVENT_TYPE_INTERACTION_STATUS } from '../src/conversation/wire.ts';
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

test('整轮收口不撤掉计划投影：计划坞只在换会话或下一次更新时变化', () => {
    // 一轮在跑时收到计划更新，随后这一轮收口：计划坞是会话事实，不能跟着消失。
    const running = applyPlan(stateWithTurn(), 0, plan('plan:1', [
        { step: '建立基线', status: 'completed' },
        { step: '审计 README', status: 'in_progress' },
    ]));
    const settled = applyFrames(
        running,
        [{
            kind: 'frame',
            subscribeId: 'interaction:turn-1',
            eventType: EVENT_TYPE_INTERACTION_STATUS,
            payloadJson: JSON.stringify('completed'),
            cursor: null,
        }],
        5_000,
    );

    assert.equal(settled.turns[0].status, 'completed', '这一轮确实收口了');
    assert.deepEqual(settled.plan, running.plan, '计划投影不随收口撤下');

    // 折叠只作用于正文区过程行：计划坞不参与折叠，下一轮仍在。
    assert.equal(applyFrames(settled, [], 6_000).plan.total, 2);
});

test('索引带来的计划快照只在还没有计划时落地，不覆盖更新的计划', () => {
    const seed = planProjectionOf('旧计划', [
        { step: '建立基线', status: 'completed' },
        { step: '审计 README', status: 'pending' },
    ]);

    // 打开会话：本视图还没有计划，快照就是首屏要显示的那一份。
    const seeded = conversationReducer(INITIAL_CONVERSATION, { type: 'plan-seed', plan: seed });
    assert.equal(seeded.plan.title, '旧计划');
    assert.equal(seeded.plan.completed, 1);
    assert.equal(seeded.plan.total, 2);

    // 回放帧或实时帧已经给出计划：快照比它旧，不许反向覆盖。
    const updated = applyPlan(stateWithTurn(), 0, plan('plan:1', [
        { step: '建立基线', status: 'completed' },
        { step: '重构导航', status: 'completed' },
    ]));
    const kept = conversationReducer(updated, { type: 'plan-seed', plan: seed });
    assert.equal(kept.plan.title, '文档事实源整理');
    assert.equal(kept.plan.completed, 2);
});

test('默认标题「计划」或空白标题不重复作为副标题渲染，自定义标题原样保留', () => {
    assert.equal(planHeading('计划'), undefined);
    assert.equal(planHeading('  计划  '), undefined);
    assert.equal(planHeading(''), undefined);
    assert.equal(planHeading('   '), undefined);
    assert.equal(planHeading('文档事实源整理'), '文档事实源整理');
});
