import test from 'node:test';
import assert from 'node:assert/strict';
import {
    EMPTY_FORM,
    formFromJob,
    requestFromForm,
    wakeError,
    wakeLabel,
    wakeText,
} from '../src/components/automation/jobForm.ts';

/*
 * 自动化任务的投递契约：表单里只有一个 wake 文本字段，只接受会话注入形态
 * `session:<channel>:<destination>[:<workspace>]`；留空落成 null，
 * 表示执行完只记录运行事实，不产生任何投递。
 */

const job = {
    name: '每日同步',
    schedule: '0 9 * * *',
    prompt: '同步数据',
    wake: 'session:desktop:workbench-1',
    workdir: null,
    skills: ['sync'],
    noAgent: false,
    script: null,
};

test('任务快照的 wake 进入表单，null 收成空串', () => {
    assert.equal(formFromJob(job).wake, 'session:desktop:workbench-1');
    assert.equal(formFromJob({ ...job, wake: null }).wake, '');
    assert.equal(formFromJob(job).workdir, '');
    assert.equal(formFromJob(job).script, '');
});

test('表单的 wake 去掉两端空白，留空落成 null', () => {
    assert.equal(
        requestFromForm({ ...EMPTY_FORM, name: '任务', wake: ' session:desktop:workbench-1:axon-code ' }).wake,
        'session:desktop:workbench-1:axon-code',
    );
    assert.equal(requestFromForm({ ...EMPTY_FORM, wake: '' }).wake, null);
    assert.equal(requestFromForm({ ...EMPTY_FORM, wake: '   ' }).wake, null);
});

test('详情文案：没有目标时写未设置', () => {
    assert.equal(wakeLabel('session:desktop:workbench-1'), 'session:desktop:workbench-1');
    assert.equal(wakeLabel(null), '未设置');
    assert.equal(wakeLabel(undefined), '未设置');
    assert.equal(wakeText('  '), '');
});

test('投递目标只接受会话注入形态', () => {
    assert.equal(wakeError(' session:desktop:workbench-1:axon-code '), null);
    assert.equal(wakeError(''), null);
    assert.match(wakeError('oc_123'), /session:/);
    assert.match(wakeError('session'), /session:/);
});

test('请求体只带 wake，不再带旧的投递字段', () => {
    const request = requestFromForm({ ...EMPTY_FORM, name: '任务' });
    assert.equal('recipientId' in request, false);
    assert.equal('threadId' in request, false);
    assert.equal('delivery' in request, false);
    assert.equal(request.wake, null);
});
