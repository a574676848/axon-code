import test from 'node:test';
import assert from 'node:assert/strict';
import { toolRowTitle } from '../src/components/conversation/toolPresentation.ts';

/** 折叠态的固定事实来源顺序：模型自述目的 → 专用参数 → 文件路径 → 命令 → 结果摘要。 */
const cell = (overrides = {}) => ({
    intent: '',
    toolKind: 'builtin',
    toolName: '',
    arguments: '',
    command: '',
    editPath: null,
    summary: '工具调用已开始',
    ...overrides,
});

test('有模型自述目的时折叠态只显示目的，不显示命令截取', () => {
    assert.equal(
        toolRowTitle(cell({ intent: '  校验代码格式  ', toolName: 'shell_command', command: 'cargo fmt --all --check' })),
        '校验代码格式',
    );
});

test('记忆检索折叠态只显示 query，不摆参数 JSON', () => {
    assert.equal(
        toolRowTitle(
            cell({
                toolName: 'builtin.memory_search',
                arguments: '{"limit":8,"query":"仓库文档重整 偏好 约束 历史决策"}',
            }),
        ),
        '仓库文档重整 偏好 约束 历史决策',
    );
});

test('技能折叠态只显示技能名', () => {
    assert.equal(
        toolRowTitle(cell({ toolName: 'invoke_skill', arguments: '{"skill_name":"open-source-docs"}' })),
        'open-source-docs',
    );
});

test('MCP 折叠态显示服务名圆点拼工具名', () => {
    assert.equal(
        toolRowTitle(
            cell({
                toolKind: 'mcp',
                toolName: 'mcp__codebase-memory-mcp__get_architecture',
                arguments: '{"project":"E-zbg-axon-code"}',
            }),
        ),
        'codebase-memory-mcp · get_architecture',
    );
});

test('委托快速代理折叠态只显示 task', () => {
    assert.equal(
        toolRowTitle(
            cell({
                toolName: 'delegate_to_fast',
                arguments: '{"run_in_background":true,"task":"目标: 只读审计 docs 全部 Markdown"}',
            }),
        ),
        '目标: 只读审计 docs 全部 Markdown',
    );
});

test('发送消息折叠态只显示 message', () => {
    assert.equal(
        toolRowTitle(cell({ toolName: 'send_message', arguments: '{"message":"请控制审计耗时并尽快返回结论"}' })),
        '请控制审计耗时并尽快返回结论',
    );
});

test('作业输出折叠态显示等待该作业的结果', () => {
    assert.equal(
        toolRowTitle(
            cell({
                toolName: 'job_output',
                arguments: '{"job_id":"1","max_output_tokens":5500,"yield_time_ms":0}',
            }),
        ),
        '等待 1 结果',
    );
});

test('新建上下文折叠态只显示 reason', () => {
    assert.equal(
        toolRowTitle(
            cell({
                toolName: 'builtin.new_context',
                arguments:
                    '{"reason":"当前阶段已完成，checkpoint 便签已保存，准备在新窗口继续"}',
            }),
        ),
        '当前阶段已完成，checkpoint 便签已保存，准备在新窗口继续',
    );
    // reason 是可选字段：缺省时回落，不凭空空造。
    assert.equal(toolRowTitle(cell({ toolName: 'builtin.new_context', arguments: '{}' })), '工具调用已开始');
});

test('查看剩余上下文收起时不追加任何内容', () => {
    assert.equal(
        toolRowTitle(
            cell({
                toolName: 'get_context_remaining',
                arguments: '{"__axon_tool_call_parse_error":"empty_arguments"}',
            }),
        ),
        null,
    );
});

test('没有专用规则时回落到可打开的文件路径，其次命令投影，最后结果摘要', () => {
    assert.equal(
        toolRowTitle(cell({ toolName: 'read_file', command: 'crates/axon-cli/src/main.rs', editPath: 'crates/axon-cli/src/main.rs' })),
        'crates/axon-cli/src/main.rs',
    );
    assert.equal(toolRowTitle(cell({ toolName: 'shell_command', command: 'cargo test' })), 'cargo test');
    assert.equal(toolRowTitle(cell({ toolName: 'list_agents' })), '工具调用已开始');
});

test('参数 JSON 被截断或字段缺失时回落，不凭空空造主文本', () => {
    assert.equal(
        toolRowTitle(cell({ toolName: 'memory_search', arguments: '{"limit":8,"query":"未完整' })),
        '工具调用已开始',
    );
    assert.equal(toolRowTitle(cell({ toolName: 'send_message', arguments: '{"target":"/root/a"}' })), '工具调用已开始');
});
