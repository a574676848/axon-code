import builtinLabels from '../../../../../axon-display/src/builtin_tool_labels.json' with { type: 'json' };
import toolRowRules from '../../../../../axon-display/src/tool_row_rules.json' with { type: 'json' };

const BUILTIN_LABELS: Readonly<Record<string, string>> = builtinLabels;
const KIND_LABELS: Record<string, string> = {
    read_file: '读取', write_file: '写入', search: builtinLabels.search, glob: '匹配',
    edit: '编辑', shell: '执行', job: '作业', mcp: 'MCP',
    invoke_skill: '技能', memory_write: '记忆写入', memory_search: '记忆检索',
    history: '历史', notes: '便签', task_tool: '任务', subagent: '子代理',
    get_context_remaining: builtinLabels.get_context_remaining,
    new_context: builtinLabels.new_context,
    list_mcp_resources: builtinLabels.list_mcp_resources,
    read_mcp_resource: builtinLabels.read_mcp_resource,
    update_plan: builtinLabels.update_plan,
};
const NAME_KINDS: Record<string, string> = {
    read_file: 'read_file', write_file: 'write_file', edit_file: 'edit',
    read: 'read_file', write: 'write_file', edit: 'edit',
    apply_patch: 'edit', memory_write: 'memory_write', memory_search: 'memory_search',
    shell: 'shell', shell_command: 'shell', exec_command: 'shell', bash: 'shell', write_stdin: 'shell',
    invoke_skill: 'invoke_skill', search: 'search', glob: 'glob', history: 'history',
};
const FAILED_STATUSES = new Set(['failed', 'error', 'rejected', 'interrupted', 'cancelled', 'canceled']);

/** 原始工具名优先区分记忆、便签与文件；MCP 保留现有前缀。 */
export function toolLabel(toolKind: string, toolName: string): string {
    const kind = toolKind.toLowerCase();
    const rawName = toolName.trim();
    if (kind === 'mcp' || rawName.startsWith('mcp__')) return 'MCP';
    const name = rawName.replace(/^builtin\./, '');
    if (Object.hasOwn(BUILTIN_LABELS, name)) return BUILTIN_LABELS[name];
    if (Object.hasOwn(NAME_KINDS, name)) return KIND_LABELS[NAME_KINDS[name]];
    // 名称存在但未识别时保留原名，避免把新工具误标成文件或便签写入。
    if (rawName) return rawName;
    return Object.hasOwn(KIND_LABELS, kind) ? KIND_LABELS[kind] : '工具';
}

export function toolFailed(status: string, exitCode: number | null): boolean {
    return (exitCode !== null && exitCode !== 0) || FAILED_STATUSES.has(status.toLowerCase());
}

/** Shell 参数单独提取命令，JSON 参数仍在输入区完整展示。 */
export function shellCommand(value: string): string | null {
    try {
        const parsed: unknown = JSON.parse(value);
        if (typeof parsed !== 'object' || parsed === null) return null;
        const args = parsed as Record<string, unknown>;
        const command = args.command ?? args.cmd;
        return typeof command === 'string' ? command : null;
    } catch {
        return null;
    }
}

/** 文件工具的参数字段可信；纯路径参数也支持无目录的文件名。 */
export function extractFilePath(value: string): string | null {
    const text = value.trim();
    if (!text) return null;
    try {
        const parsed: unknown = JSON.parse(text);
        if (typeof parsed === 'object' && parsed !== null) {
            for (const key of ['path', 'file_path', 'filePath', 'file', 'filename']) {
                const candidate = (parsed as Record<string, unknown>)[key];
                if (typeof candidate === 'string' && candidate.trim()) return candidate.trim();
            }
        }
        return null;
    } catch {
        return !/[\r\n]/.test(text) && !text.startsWith('{') ? text : null;
    }
}

/** JSON 只调整缩进，Shell 与其他文本不改写原始语义。 */
export function formatToolContent(value: string): { code: string; language: string | null } {
    const text = value.trim();
    if (text.startsWith('{') || text.startsWith('[')) {
        try {
            return { code: JSON.stringify(JSON.parse(text), null, 2), language: 'json' };
        } catch {
            // 流式或截断的 JSON 保持原文。
        }
    }
    return { code: value, language: null };
}

/** 折叠态专用规则：从调用参数里取出这一行最该看见的那一项事实。 */
interface ToolRowRule {
    /** 依次尝试的参数名；取第一个非空值。 */
    fields?: string[];
    /** 命中后的文案模板，`{value}` 替换为字段值。 */
    template?: string;
    /** 参数对用户没有信息量：收起时不追加任何内容。 */
    silent?: boolean;
}

/*
 * 规则表与 TUI、飞书渠道共用 `axon-display` 的 `tool_row_rules.json`：
 * 这些工具的参数 JSON 里只有一项对用户有意义，其余是分页、预算一类执行细节，
 * 收起时摆了也只是噪声。
 */
const TOOL_ROW_RULES: Readonly<Record<string, ToolRowRule>> = toolRowRules.targets;

/** 工具名去掉命名空间与大小写差异后的裸名。 */
function bareToolName(toolName: string): string {
    return toolName.trim().replace(/^builtin\./, '').toLowerCase();
}

/** MCP 工具名的服务名与工具名，形态为 `mcp__server__tool`。 */
function mcpToolParts(toolName: string): { server: string; tool: string } | null {
    const match = /^mcp__(.+?)__(.+)$/.exec(toolName.trim());
    return match === null ? null : { server: match[1], tool: match[2] };
}

/** MCP 折叠态：服务名圆点拼工具名；不是 `mcp__` 形态时不接管这一行。 */
function mcpRowTitle(toolKind: string, toolName: string): string | null {
    if (toolKind.toLowerCase() !== 'mcp' && !toolName.trim().startsWith('mcp__')) return null;
    const parts = mcpToolParts(toolName);
    return parts === null ? null : `${parts.server} · ${parts.tool}`;
}

/** 逐字段取第一个非空值并折成单行；参数缺失、非字符串或 JSON 未完整时返回 null。 */
function argumentField(argumentsJson: string, fields: string[]): string | null {
    let parsed: unknown;
    try {
        parsed = JSON.parse(argumentsJson);
    } catch {
        return null;
    }
    if (typeof parsed !== 'object' || parsed === null) return null;
    const args = parsed as Record<string, unknown>;
    for (const field of fields) {
        const value = args[field];
        if (typeof value === 'number') return String(value);
        if (typeof value === 'string' && value.trim().length > 0) {
            return value.trim().split(/\s+/).join(' ');
        }
    }
    return null;
}

/**
 * 工具行折叠态的主文本。
 *
 * 顺序固定：模型自述的目的 → 该工具最该看见的那一项参数 → 文件路径 → 命令投影 → 结果摘要。
 * 返回 null 表示这一行收起时不追加任何内容（例如查看剩余上下文：参数本身没有信息量），
 * 此时行头只留图标与类别前缀。原始参数在展开态完整可见，事实不丢。
 */
export function toolRowTitle(cell: {
    intent: string;
    toolKind: string;
    toolName: string;
    arguments: string;
    command: string;
    editPath: string | null;
    summary: string;
}): string | null {
    const intent = cell.intent.trim();
    if (intent.length > 0) return intent;
    const mcp = mcpRowTitle(cell.toolKind, cell.toolName);
    if (mcp !== null) return mcp;
    const rule = TOOL_ROW_RULES[bareToolName(cell.toolName)];
    if (rule?.silent === true) return null;
    if (rule?.fields !== undefined) {
        const value = argumentField(cell.arguments, rule.fields);
        if (value !== null) {
            return rule.template === undefined ? value : rule.template.replace('{value}', value);
        }
    }
    return cell.editPath ?? (cell.command.length > 0 ? cell.command : cell.summary);
}