import test from 'node:test';
import assert from 'node:assert/strict';
import { failureBanner } from '../src/conversation/failureBanner.ts';

test('主会话的失败仍然提示重新发送', () => {
    const banner = failureBanner({ lane: 'primary', status: 'failed' });
    assert.equal(banner.text, '本轮执行失败，可重新发送这条消息。');
});

test('主会话被后端拒绝时给凭据与模型提示', () => {
    const banner = failureBanner({ lane: 'primary', status: 'rejected' });
    assert.equal(banner.text, '本轮被后端拒绝，请检查模型与凭据配置后重试。');
});

test('子代理车道不再提示重新发送，改为父轮归因', () => {
    const banner = failureBanner({ lane: 'subagent', status: 'failed', interrupted: true });
    assert.equal(banner.text, '父轮失败导致本子代理被中断，请在上层会话重发。');
});

test('子代理自身失败与被中断给不同结论', () => {
    const failed = failureBanner({ lane: 'subagent', status: 'failed', interrupted: false });
    const rejected = failureBanner({ lane: 'subagent', status: 'rejected' });
    assert.equal(failed.text, '本子代理执行失败，请在上层会话查看原因后重发。');
    assert.equal(rejected.text, '本子代理请求被后端拒绝，请在上层会话检查模型与凭据配置。');
});

test('被中断的判定缺省按失败处理，不给出误导性重发提示', () => {
    const banner = failureBanner({ lane: 'subagent', status: 'failed' });
    assert.equal(banner.text, '本子代理执行失败，请在上层会话查看原因后重发。');
    assert.equal(banner.text.includes('重新发送这条消息'), false);
});
