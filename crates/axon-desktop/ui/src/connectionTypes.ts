/*
 * 连接档案的界面类型：与 Rust 侧的 IPC 载荷逐字段对应。
 *
 * 这些类型只描述「界面看到了什么」，不含任何凭据：Key 从不出现在这些结构里，
 * 也不会被放进 React 全局状态。
 */

/** 一条能力位的展示形态。 */
export interface CapabilityView {
  key: string;
  label: string;
  enabled: boolean;
  /** 是否应当隐藏：只有「已知且为 false」才隐藏。 */
  hidden: boolean;
}

/** 健康结论：状态点档位加一句可执行的说明。 */
export interface ConnectionHealthView {
  dot: 'idle' | 'ongoing' | 'done' | 'error' | 'warning';
  label: string;
  detail: string;
}

/** 档案清单的一条。 */
export interface ProfileView {
  id: string;
  label: string;
  mode: string;
  endpoint: string;
  active: boolean;
  credential: string;
  healthLabel: string;
  healthDot: 'idle' | 'ongoing' | 'done' | 'error' | 'warning';
}

/** 连接快照：胶囊与连接分区共用的唯一事实。 */
export interface ConnectionSnapshot {
  activeId: string | null;
  activeLabel: string;
  mode: string;
  isCloud: boolean;
  endpoint: string;
  readOnly: boolean;
  onboardingVersion: number;
  needsOnboarding: boolean;
  notice: string | null;
  health: ConnectionHealthView;
  backendVersion: string | null;
  latencyMs: number | null;
  clockOffsetMs: number | null;
  clockVerified: boolean;
  capsKnown: boolean;
  backendOutdated: boolean;
  capabilities: CapabilityView[];
  superAdmin: boolean;
  uploadLimitBytes: number;
  plaintextPrivate: boolean;
  secretStoreAvailable: boolean;
  secretStoreReason: string | null;
  profiles: ProfileView[];
  keyManagementVisible: boolean;
}

/** 试连接的输入。 */
export interface ConnectionTestInput {
  address: string;
  apiKey?: string;
}

/** 试连接的结论分类。 */
export type ConnectionConclusion =
  | 'ok'
  | 'credential'
  | 'certificate'
  | 'unreachable'
  | 'address'
  | 'protocol';

/** 试连接的回执。 */
export interface ConnectionTestResult {
  conclusion: ConnectionConclusion;
  message: string;
  endpoint: string | null;
  /** 事件流地址：按后端自报的端口校正后的结果。 */
  wsEndpoint: string | null;
  /** 事件流端口的探测结论：ok / wrongPort / credential / unreachable / notTested。 */
  wsConclusion: 'ok' | 'wrongPort' | 'credential' | 'unreachable' | 'notTested';
  /** 事件流端口结论的中文说明。 */
  wsMessage: string;
  backendVersion: string | null;
  latencyMs: number | null;
  capabilities: CapabilityView[];
  certificate: 'trusted' | 'invalid' | 'unknown';
  plaintextPrivate: boolean;
  backendOutdated: boolean;
}

/** 切档结果。 */
export interface SwitchOutcome {
  started: boolean;
  message: string;
}

/** 一次惰性探测的结论。 */
export interface ProfileProbeView {
  id: string;
  conclusion:
    | 'ok'
    | 'unreachable'
    | 'credential'
    | 'certificate'
    | 'outdated'
    | 'overBudget';
  message: string;
  latencyMs: number | null;
}

/**
 * 一把 Gateway Key 的展示形态。
 *
 * 只有标签、两个时间与前 6 位指纹：明文与摘要永远不经这条通道回传。
 */
export interface KeyView {
  id: string;
  label: string;
  createdAtMs: number;
  /** 从未用于鉴权时为 null。 */
  lastUsedAtMs: number | null;
  fingerprint: string;
}

/**
 * 签发回执。
 *
 * 明文只在这一份回执里出现一次，关掉签发弹窗后无法再看，因此它不进入任何全局状态。
 */
export interface KeyIssueReceipt {
  id: string;
  label: string;
  plaintext: string;
  createdAtMs: number;
}
