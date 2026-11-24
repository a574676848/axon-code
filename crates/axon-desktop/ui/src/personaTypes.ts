/** 人格内核的人设表达基线。 */
export interface PersonaVoice {
  warmth: number;
  formality: number;
  humor: string;
  verbosity: string;
}

/** 人格内核：由 SOUL.md 解析得到。 */
export interface PersonaSoul {
  file: string;
  ready: boolean;
  error?: string | null;
  schema?: string;
  identity?: { name: string; address: string; role: string };
  backstoryChars?: number;
  invariants?: string[];
  traits?: string[];
  voice?: PersonaVoice;
  rules?: PersonaRule[];
  notesChars?: number;
}

/** 按 owner 持久化的人格状态。 */
export interface PersonaState {
  revision: number;
  warmthOffset: number;
  familiarity: number;
  userAffect: string;
  fatigueLevel: string;
  failureStreak: number;
  lastCause: string;
  driftGuard: string | null;
  updatedAtMs: number;
}

/** 一条人格转移记录。 */
export interface PersonaTransition {
  revision: number;
  cause: string;
  occurredAtMs: number;
}

/** 已发布的习得风格。 */
export interface PersonaStyle {
  revision: number;
  entries: { styleId: string; text: string; publishedAtMs: number | null }[];
}

/** 网关返回的人格状态摘要。 */
export interface PersonaSummary {
  soul: PersonaSoul;
  owner: { present: boolean; owner?: string; error?: string };
  state: PersonaState | null;
  transitions: PersonaTransition[];
  style: PersonaStyle;
  database: { ready: boolean; schema: string; bytes: number };
}

/** 形象资产：网关返回的 PNG 的 Base64 视图。 */
export interface PersonaPortrait {
  file: string;
  mime: string;
  base64: string;
}

/** 一条场景规则：声明在什么场景下把表达基线改到哪。 */
export interface PersonaRule {
  mode?: string | null;
  audience?: string | null;
  userAffect?: string | null;
  domain?: string | null;
  fatigueLevel?: string | null;
  warmth?: number | null;
  formality?: number | null;
  humor?: string | null;
  verbosity?: string | null;
}