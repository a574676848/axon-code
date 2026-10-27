/*
 * 语音播报的偏好键。
 *
 * 键名集中在这里：散落到组件里之后，改名要满仓库找，读到旧键也无人兜底。
 * 全局开关与会话开关各占一个键，会话键按「工作区 + 会话」拼成，
 * 因此一条会话的开关不会牵动任何别的会话。
 */

/** 主题与语音共用同一个本地命名空间前缀。 */
const PREFIX = 'axon.voice.auto-read';

export const VOICE_STORAGE_KEYS = {
  /** 全局开关：新回复是否默认自动朗读。 */
  global: PREFIX,
  /** 会话开关：一条会话一个键，入参是工作区与会话拼成的车道键。 */
  lane: (laneKey: string): string => `${PREFIX}.lane.${laneKey}`,
} as const;
