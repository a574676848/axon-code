/*
 * 音色预设：把网关清单与「跟随人格」合成同一列可选项。
 *
 * 清单只由网关回答（`voice_presets`），每个预设自带可用性：后端判定不可用时
 * 该预设不可选，界面必须标注原因而不是让它看起来能用。第一项「跟随人格」不在
 * 网关清单里，是本机固定的一条：选中它时不带预设 id，后端按人格融合选音色。
 *
 * 这一层只做两件事：校验回执形状，算出谁被选中。选择本身落在
 * `preferences.ts` 的本地存储里（键 `axon.voice.preset`，空串表示跟随人格）。
 */

/** 网关给出的一个音色预设。 */
export interface VoicePreset {
  /** 稳定 id，朗读合成时原样回传。 */
  id: string;
  /** 设置页显示名。 */
  name: string;
  /** 依赖的模型是否已安装；不可用的预设不能选中。 */
  available: boolean;
  /** 网关侧的默认预设；界面仍以「跟随人格」为默认。 */
  default: boolean;
}

/** `voice_presets` 的回执形状。 */
export interface VoicePresetsPayload {
  presets: VoicePreset[];
}

/** 一行可选项：网关清单项，或本机固定的「跟随人格」。 */
export interface VoicePresetRow {
  id: string;
  name: string;
  /** 不可用项在界面上禁用并标注原因。 */
  available: boolean;
  checked: boolean;
}

/** 跟随人格的 id：空串，合成时不带预设。 */
export const FOLLOW_PERSONA_ID = '';

/** 固定排在第一项的那一行。 */
const FOLLOW_PERSONA_ROW = {
  id: FOLLOW_PERSONA_ID,
  name: '跟随人格（默认）',
  available: true,
} as const;

/**
 * 校验回执并归一成预设清单。
 *
 * 形状不对的项直接丢掉：宁可少一行，也不拿脏数据渲染一个点不动的选项。
 * `available` 只有明确的 `false` 才算不可用，字段缺失按可用处理。
 */
export function parseVoicePresets(raw: unknown): VoicePreset[] {
  const record = (raw ?? {}) as { presets?: unknown };
  const items = Array.isArray(record.presets) ? record.presets : [];
  const seen = new Set<string>();
  const presets: VoicePreset[] = [];
  for (const item of items) {
    if (item === null || typeof item !== 'object') continue;
    const entry = item as Record<string, unknown>;
    const id = typeof entry.id === 'string' ? entry.id.trim() : '';
    const name = typeof entry.name === 'string' ? entry.name.trim() : '';
    if (id.length === 0 || name.length === 0 || seen.has(id)) continue;
    seen.add(id);
    presets.push({
      id,
      name,
      available: entry.available !== false,
      default: entry.default === true,
    });
  }
  return presets;
}

/**
 * 「跟随人格」加清单，并算出选中态。
 *
 * 只有既在清单里又可用的一项才算选中：存下来的 id 若已经不可用（模型被移除、
 * 后端换了预设表），选择落回「跟随人格」，界面与实际合成请求不会各说一套。
 */
export function voicePresetRows(
  presets: readonly VoicePreset[],
  selectedId: string,
): VoicePresetRow[] {
  const selectable = presets.some(
    (preset) => preset.id === selectedId && preset.available,
  );
  const selected = selectable ? selectedId : FOLLOW_PERSONA_ID;
  const rows: VoicePresetRow[] = [{ ...FOLLOW_PERSONA_ROW, checked: selected === FOLLOW_PERSONA_ID }];
  for (const preset of presets) {
    rows.push({
      id: preset.id,
      name: preset.name,
      available: preset.available,
      checked: preset.id === selected,
    });
  }
  return rows;
}
