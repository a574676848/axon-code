import test from 'node:test';
import assert from 'node:assert/strict';
import {
    failureHeadline,
    failureReportFromPayload,
    failureReportFromRuntimeFailure,
    parseFailureReport,
} from '../src/conversation/failureReport.ts';

/** 生产同形：自然语言前缀 + run_failed 载荷（真机上就是这条把界面刷成了 JSON）。 */
const PREFIX = '本轮执行未完成：Provider turn 执行失败，退出码 1：';
const FULL = `${PREFIX}{"event":"run_failed","status":"failed","sessionId":"3a2b26ed","provider":"longcat","model":"LongCat-2.5-Preview","httpStatus":400,"content":null,"failure":{"category":"provider_transport","code":"invalid_parameter","providerProtocol":"anthropic","redactedMessage":"Image too small (area=64px), minimum required area: >256px","httpStatus":400,"retryable":true,"replaySafe":true,"attemptCount":1}}`;
/** 真机上这条载荷被 CLI 截断到 600 字符，整段 JSON 已不可解析。 */
const TRUNCATED = `${FULL.slice(0, 600)}...`;

test('完整载荷给出结论、类别与可重试', () => {
    const report = parseFailureReport(FULL);
    assert.ok(report !== null);
    assert.equal(report.reason, 'Image too small (area=64px), minimum required area: >256px');
    assert.equal(report.code, 'invalid_parameter');
    assert.equal(report.category, 'provider_transport');
    assert.equal(report.provider, 'longcat');
    assert.equal(report.model, 'LongCat-2.5-Preview');
    assert.equal(report.httpStatus, 400);
    assert.equal(report.retryable, true);
    assert.equal(failureHeadline(report), '请求未被 Provider 接受');
});

test('被截断的载荷仍然解出同样的事实，而不是把半截 JSON 甩给用户', () => {
    assert.throws(() => JSON.parse(TRUNCATED.slice(TRUNCATED.indexOf('{'))), '这条用例必须是真截断');
    const report = parseFailureReport(TRUNCATED);
    assert.ok(report !== null);
    assert.equal(report.reason, 'Image too small (area=64px), minimum required area: >256px');
    assert.equal(report.code, 'invalid_parameter');
    assert.equal(report.provider, 'longcat');
    assert.equal(report.model, 'LongCat-2.5-Preview');
    assert.equal(report.httpStatus, 400);
    assert.equal(report.retryable, true);
    assert.equal(report.raw.length > 0, true, '原始回执保留给展开查看');
});

test('终态载荷里的结构化失败优先于文本解析', () => {
    const report = failureReportFromPayload({
        provider: 'longcat',
        model: 'LongCat-2.5-Preview',
        failure: {
            category: 'provider_transport',
            code: 'invalid_parameter',
            redactedMessage: '图片过小',
            httpStatus: 400,
            retryable: true,
        },
    });
    assert.ok(report !== null);
    assert.equal(report.reason, '图片过小');
    assert.equal(report.retryable, true);
    assert.equal(report.raw, '', '结构化来源没有原始文本可展开');
});

test('不是失败回执的正文不产生卡片', () => {
    assert.equal(parseFailureReport('收到'), null);
    assert.equal(parseFailureReport(''), null);
    assert.equal(parseFailureReport('用时 8s · {这不是失败载荷}'), null);
    assert.equal(failureReportFromPayload({ response: '收到' }), null);
    assert.equal(failureReportFromPayload(null), null);
});

test('认不出的错误码回落到通用标题', () => {
    assert.equal(failureHeadline({
        reason: 'something', category: 'provider', code: 'weird_code',
        provider: '', model: '', httpStatus: null, retryable: false, raw: '',
    }), '本轮执行未完成');
});

test('Provider 错误信封直接解出结论与错误码', () => {
    const report = parseFailureReport(
        '本轮执行未完成：{"code":"UNKNOWN_FIELD","message":"未知请求字段：reasoning_summary","data":{"field":"reasoning_summary"},"traceId":"trace_6aa19ae6"}；reason=provider_streaming_runner:provider_error（runtime_failed）',
    );
    assert.ok(report !== null);
    assert.equal(report.reason, '未知请求字段：reasoning_summary');
    assert.equal(report.code, 'UNKNOWN_FIELD');
    assert.equal(failureHeadline(report), '请求字段不被 Provider 接受');
});

test('运行时失败事件映射成同一张卡的字段', () => {
    const report = failureReportFromRuntimeFailure({
        category: 'provider_transport',
        code: 'upstream_503',
        redactedMessage: '上游返回 503',
        httpStatus: 503,
        retryable: true,
    });
    assert.ok(report !== null);
    assert.equal(report.reason, '上游返回 503');
    assert.equal(report.httpStatus, 503);
    assert.equal(report.retryable, true);
    assert.equal(failureHeadline(report), 'Provider 请求失败');
    assert.equal(failureReportFromRuntimeFailure({}), null);
});

test('错误码到标题的映射覆盖各家族，认不出时回落到通用标题', () => {
    const headline = (code) =>
        failureHeadline({
            reason: 'x', category: '', code, provider: '', model: '', httpStatus: null, retryable: false, raw: '',
        });
    assert.equal(headline('unknown_field'), '请求字段不被 Provider 接受');
    assert.equal(headline('context_length_exceeded'), '上下文超出窗口');
    assert.equal(headline('rate_limit_exceeded'), 'Provider 限流');
    assert.equal(headline('insufficient_quota'), 'Provider 额度不足');
    assert.equal(headline('authentication_error'), '凭据无效');
    assert.equal(headline('permission_denied'), '凭据权限不足');
    assert.equal(headline('request_timeout'), 'Provider 响应超时');
    assert.equal(headline('internal_error'), 'Provider 内部错误');
    assert.equal(headline('image_too_small'), '图片不符合 Provider 要求');
    assert.equal(headline('model_not_found'), '模型不可用');
    assert.equal(headline('weird_code'), '本轮执行未完成');
});
