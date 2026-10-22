/*
 * 失败回执的解读：把 `run_failed` 这类结构化失败说成一张卡片能用的字段。
 *
 * 两个来源都要认：终态载荷里已经整理好的 `failure` 字段（新路径），以及
 * 历史会话里被写进正文的那段 JSON（旧路径）。旧路径的载荷可能被上游按长度截断，
 * 因此解析失败时按字段名就地扫一遍字符串，而不是把半截 JSON 甩给用户。
 */

/** 解读后的失败事实。 */
export interface FailureReport {
  /** 一行结论，取自 Provider 的脱敏说明。 */
  reason: string;
  /** 失败类别与错误码，用于给结论定性。 */
  category: string;
  code: string;
  /** Provider 与模型；未知时为空串。 */
  provider: string;
  model: string;
  /** Provider 返回的 HTTP 状态码；未知时为 null。 */
  httpStatus: number | null;
  /** 是否可重试。 */
  retryable: boolean;
  /** 渠道尝试次数；未知时为 null。 */
  attemptCount: number | null;
  /** 触发失败的 Provider 终态事件类型（如 `response.failed`）；未知时为空串。 */
  providerEventType: string;
  /** Provider 或边缘网关返回的请求 ID；未知时为空串。 */
  upstreamRequestId: string;
  /** 原始回执文本；没有结构可展示时为空串。 */
  raw: string;
}

/** 面向用户的标题：按错误码给一句人话，认不出时按类别回落，最后才用通用标题。 */
export function failureHeadline(report: FailureReport): string {
  const code = report.code.toLowerCase();
  // 协议与参数类：请求本身没被接受，改协议字段或参数才过得去。
  if (code === 'unknown_field' || code === 'unrecognized_field') return '请求字段不被 Provider 接受';
  if (code === 'invalid_parameter' || code === 'invalid_request_error') return '请求未被 Provider 接受';
  if (code === 'unsupported_parameter') return '该 Provider 不支持请求里的参数';
  // 容量与配额类。
  if (code === 'context_overflow' || code === 'context_length_exceeded') return '上下文超出窗口';
  if (code === 'rate_limited' || code === 'rate_limit_exceeded' || code === 'too_many_requests') {
    return 'Provider 限流';
  }
  if (code === 'insufficient_quota' || code === 'quota_exceeded') return 'Provider 额度不足';
  // 凭据与权限类。
  if (code === 'unauthorized' || code === 'invalid_api_key' || code === 'authentication_error') {
    return '凭据无效';
  }
  if (code === 'permission_denied' || code === 'forbidden') return '凭据权限不足';
  // 网络与上游类。
  if (code === 'timeout' || code === 'request_timeout') return 'Provider 响应超时';
  if (code === 'server_error' || code === 'internal_error') return 'Provider 内部错误';
  if (code === 'invalid_image' || code === 'image_too_small') return '图片不符合 Provider 要求';
  if (code === 'model_not_found') return '模型不可用';
  if (report.category.toLowerCase() === 'provider_transport') return 'Provider 请求失败';
  return '本轮执行未完成';
}

/** Provider 直接回的 `{code, message, data, traceId}` 信封。 */
const PROVIDER_ENVELOPE_KEYS = ['code', 'message'];

/**
 * 从 Provider 错误信封解读失败；不是信封时返回 null。
 *
 * 这类回执没有 `failure` 包装层（真机样本：`{"code":"UNKNOWN_FIELD","message":
 * "未知请求字段：reasoning_summary","data":{"field":"reasoning_summary"},
 * "traceId":"trace_..."}`），因此按 `code` + `message` 这一对判形。
 */
function reportFromProviderEnvelope(payload: Record<string, unknown>, raw: string): FailureReport | null {
  if (!PROVIDER_ENVELOPE_KEYS.every((key) => typeof payload[key] === 'string')) return null;
  const code = stringOf(payload.code);
  const message = stringOf(payload.message);
  if (code.length === 0 && message.length === 0) return null;
  return {
    reason: message.length > 0 ? message : code,
    category: stringOf(payload.category),
    code,
    provider: stringOf(payload.provider),
    model: stringOf(payload.model),
    httpStatus: numberOrNull(payload.httpStatus) ?? numberOrNull(payload.status),
    retryable: payload.retryable === true,
    attemptCount: numberOrNull(payload.attemptCount),
    providerEventType: stringOf(payload.providerEventType),
    upstreamRequestId: stringOf(payload.upstreamRequestId),
    raw,
  };
}

/** 从文本里解读失败回执；认不出结构时返回 null。 */
export function parseFailureReport(text: string): FailureReport | null {
  const source = text.trim();
  if (source.length === 0) return null;
  const raw = extractJsonObject(source);
  if (raw === null) return null;
  const parsed = tryParse(raw);
  if (parsed !== null) {
    const report = reportFromPayload(parsed, raw);
    if (report !== null) return report;
  }
  // 载荷被截断或整体不可解析：字段本身往往还在，按名扫一遍仍然能给出一句人话。
  const reason = scanStringField(raw, 'message') ?? scanStringField(raw, 'redactedMessage');
  const code = scanStringField(raw, 'code');
  if (reason === null && code === null) return null;
  return {
    reason: reason ?? code ?? '',
    category: scanStringField(raw, 'category') ?? '',
    code: code ?? '',
    provider: scanStringField(raw, 'provider') ?? '',
    model: scanStringField(raw, 'model') ?? '',
    httpStatus: scanNumberField(raw, 'httpStatus'),
    retryable: scanBooleanField(raw, 'retryable') ?? false,
    attemptCount: scanNumberField(raw, 'attemptCount'),
    providerEventType: scanStringField(raw, 'providerEventType') ?? '',
    upstreamRequestId: scanStringField(raw, 'upstreamRequestId') ?? '',
    raw,
  };
}

/**
 * 取出文本里的第一个完整 JSON 对象。
 *
 * 失败回执后面常跟着别的说明（真机样本：`本轮执行未完成：{...}；reason=...`），
 * 按「第一个 `{` 到末尾」切会把尾巴一起吃进去，整段 JSON 因此解析失败。
 * 这里按括号配平取，字符串内的括号与转义不参与计数。
 */
function extractJsonObject(text: string): string | null {
  const start = text.indexOf('{');
  if (start < 0) return null;
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let index = start; index < text.length; index += 1) {
    const character = text[index];
    if (inString) {
      if (escaped) escaped = false;
      else if (character === '\\') escaped = true;
      else if (character === '"') inString = false;
      continue;
    }
    if (character === '"') {
      inString = true;
      continue;
    }
    if (character === '{') depth += 1;
    else if (character === '}') {
      depth -= 1;
      if (depth === 0) return text.slice(start, index + 1);
    }
  }
  // 没配平（载荷被截断）：交给扫描路径，保留半截文本。
  return text.slice(start);
}

/** 从终态载荷里取结构化失败；没有时返回 null。 */
export function failureReportFromPayload(payload: unknown): FailureReport | null {
  if (payload === null || typeof payload !== 'object') return null;
  const failure = (payload as { failure?: unknown }).failure;
  if (failure === null || typeof failure !== 'object') return null;
  const record = failure as Record<string, unknown>;
  const fields = payload as Record<string, unknown>;
  const reason = stringOf(record.redactedMessage);
  const code = stringOf(record.code);
  if (reason.length === 0 && code.length === 0) return null;
  return {
    reason: reason.length > 0 ? reason : code,
    category: stringOf(record.category),
    code,
    provider: stringOf(fields.provider),
    model: stringOf(fields.model),
    httpStatus: numberOrNull(record.httpStatus) ?? numberOrNull(fields.httpStatus),
    retryable: record.retryable === true,
    attemptCount: numberOrNull(record.attemptCount),
    providerEventType: stringOf(record.providerEventType),
    upstreamRequestId: stringOf(record.upstreamRequestId),
    raw: '',
  };
}

function tryParse(text: string): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(text);
    return value !== null && typeof value === 'object' ? (value as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

function reportFromPayload(payload: Record<string, unknown>, raw: string): FailureReport | null {
  const failure =
    payload.failure !== null && typeof payload.failure === 'object'
      ? (payload.failure as Record<string, unknown>)
      : null;
  if (failure === null) {
    // 没有 `failure` 包装层时，可能是 Provider 直接回的 `{code, message}` 信封。
    return reportFromProviderEnvelope(payload, raw);
  }
  const reason = stringOf(failure.redactedMessage);
  const code = stringOf(failure.code);
  if (reason.length === 0 && code.length === 0) return null;
  return {
    reason: reason.length > 0 ? reason : code,
    category: stringOf(failure.category),
    code,
    provider: stringOf(payload.provider),
    model: stringOf(payload.model),
    httpStatus: numberOrNull(failure.httpStatus) ?? numberOrNull(payload.httpStatus),
    retryable: failure.retryable === true,
    attemptCount: numberOrNull(failure.attemptCount),
    providerEventType: stringOf(failure.providerEventType),
    upstreamRequestId: stringOf(failure.upstreamRequestId),
    raw,
  };
}

/** 从运行时失败事件解读失败：字段已经是结构化的，直接映射。 */
export function failureReportFromRuntimeFailure(failure: {
  category?: string;
  code?: string;
  redactedMessage?: string;
  httpStatus?: number | null;
  retryable?: boolean;
  attemptCount?: number | null;
  providerEventType?: string;
  upstreamRequestId?: string;
}): FailureReport | null {
  const code = (failure.code ?? '').trim();
  const message = (failure.redactedMessage ?? '').trim();
  if (code.length === 0 && message.length === 0) return null;
  return {
    reason: message.length > 0 ? message : code,
    category: (failure.category ?? '').trim(),
    code,
    provider: '',
    model: '',
    httpStatus: failure.httpStatus ?? null,
    retryable: failure.retryable === true,
    attemptCount: failure.attemptCount ?? null,
    providerEventType: (failure.providerEventType ?? '').trim(),
    upstreamRequestId: (failure.upstreamRequestId ?? '').trim(),
    raw: '',
  };
}

function stringOf(value: unknown): string {
  return typeof value === 'string' ? value.trim() : '';
}

function numberOrNull(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/** 扫一个字符串字段：`"<field>":"…"`，按最小转义集还原。 */
function scanStringField(text: string, field: string): string | null {
  const needle = `"${field}":"`;
  const start = text.indexOf(needle);
  if (start < 0) return null;
  let value = '';
  let escaped = false;
  for (const character of text.slice(start + needle.length)) {
    if (escaped) {
      value += character === 'n' ? '\n' : character === 't' ? '\t' : character;
      escaped = false;
      continue;
    }
    if (character === '\\') {
      escaped = true;
      continue;
    }
    if (character === '"') break;
    value += character;
  }
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

function scanNumberField(text: string, field: string): number | null {
  const needle = `"${field}":`;
  const start = text.indexOf(needle);
  if (start < 0) return null;
  let digits = '';
  for (const character of text.slice(start + needle.length)) {
    if (character >= '0' && character <= '9') {
      digits += character;
      continue;
    }
    break;
  }
  return digits.length > 0 ? Number(digits) : null;
}

function scanBooleanField(text: string, field: string): boolean | null {
  const needle = `"${field}":`;
  const start = text.indexOf(needle);
  if (start < 0) return null;
  const rest = text.slice(start + needle.length);
  if (rest.startsWith('true')) return true;
  if (rest.startsWith('false')) return false;
  return null;
}
