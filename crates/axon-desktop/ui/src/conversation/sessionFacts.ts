/*
 * 会话能力事实：由网关在历史索引页里随正文一起下发。
 *
 * 前端此前只能按 `channel` 推断「这条会话能不能发消息、能不能中断、能不能答问」，推断在
 * 目标看板与子代理上必然出错。服务端把判断权收回后（`session` 块），这里只做一件事：
 * 把载荷形状翻译成一个可判空的纯数据对象，让界面读字段而不是猜来源。
 */

/** 会话类型；与服务端 `sessions.kind` 同值。 */
export type SessionKind = 'primary' | 'goal' | 'cron' | 'subagent';

/** 会话来源；一条会话只有一种来源，不随轮次变化。 */
export type SessionOrigin = 'human' | 'goal' | 'cron' | 'subagent';

/** 网关下发的会话能力与身份事实。 */
export interface SessionFacts {
  kind: SessionKind | string;
  origin: SessionOrigin | string;
  /** 这条会话是否可以由用户提交消息；后台驱动的会话恒为 false。 */
  writable: boolean;
  /** 当前轮次是否可以中断。 */
  canInterrupt: boolean;
  /** 这条会话是否可以作答；后台链不注入提问端口，因此只有主会话为 true。 */
  canAnswerQuestions: boolean;
  title: string;
  provider: string;
  model: string;
  modelName: string;
  interactionCount: number;
  latestInteractionId: string;
}

const asString = (value: unknown): string => (typeof value === 'string' ? value : '');
const asBoolean = (value: unknown): boolean => value === true;
const asCount = (value: unknown): number =>
  typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : 0;

/**
 * 解析 `session` 块；消息体里没有这一块时返回 null。
 *
 * 返回 null 与「返回一份全 false 的取值」是两回事：前者表示服务端没有给出结论（旧后端、
 * 或不含该块的响应），界面据此退回入口声明；后者表示服务端明确判定不可写。混用会让
 * 一次缺字段的响应把可写会话锁死。
 */
export function sessionFactsOf(raw: unknown): SessionFacts | null {
  if (raw === null || typeof raw !== 'object') return null;
  const value = raw as Record<string, unknown>;
  const kind = asString(value.kind);
  if (kind === '') return null;
  return {
    kind,
    origin: asString(value.origin),
    writable: asBoolean(value.writable),
    canInterrupt: asBoolean(value.canInterrupt),
    canAnswerQuestions: asBoolean(value.canAnswerQuestions),
    title: asString(value.title),
    provider: asString(value.provider),
    model: asString(value.model),
    modelName: asString(value.modelName),
    interactionCount: asCount(value.interactionCount),
    latestInteractionId: asString(value.latestInteractionId),
  };
}
