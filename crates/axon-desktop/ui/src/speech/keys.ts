/*
 * 语音播报的偏好键。
 *
 * 键名集中在这里：散落到组件里之后，改名要满仓库找，读到旧键也无人兜底。
 * 全局开关与会话开关各占一个键，会话键按「工作区 + 会话」拼成，
 * 因此一条会话的开关不会牵动任何别的会话。
 * 音色预设独立一个键：空串表示跟随人格，由后端按人格融合选音色。
 */

/** 主题与语音共用同一个本地命名空间前缀。 */
const PREFIX = 'axon.voice.auto-read';

/** 音色预设：与自动朗读开关同级，但落在自己的键上，两者互不牵连。 */
const PRESET_KEY = 'axon.voice.preset';

export const VOICE_STORAGE_KEYS = {
  /** 全局开关：新回复是否默认自动朗读。 */
  global: PREFIX,
  /** 会话开关：一条会话一个键，入参是工作区与会话拼成的车道键。 */
  lane: (laneKey: string): string => `${PREFIX}.lane.${laneKey}`,
  /** 音色预设：空串表示跟随人格。 */
  preset: PRESET_KEY,
} as const;
