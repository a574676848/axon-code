import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, RetryIcon, SpinnerIcon } from '../../icons';
import { IconButton } from '../ui/controls';
import type { PersonaPortrait, PersonaRule, PersonaSummary } from '../../personaTypes';
import * as ipc from '../../ipc';
import { Banner, Chip, EmptyHint, ListRow, LoadingState, Panel, Text } from '../ui/surfaces';
import { VoicePresetSettings } from './VoicePresetSettings';

/*
 * 人设：只呈现单一事实源（SOUL.md 与已发布的习得风格）。
 *
 * 运行读数（状态版本、温度偏移、失败连击、转移日志、库体积）属于支撑数据，
 * 不在这块面板展示；这里只回答「她是谁、怎么说话、守什么规矩」。
 *
 * 唯一的例外是「音色」：它是本机偏好（localStorage 的 axon.voice.preset），
 * 不写回 SOUL.md，因此紧贴表达基线单独成行并自带读写，不与人设状态数据混排。
 */

const HUMOR_LABEL: Record<string, string> = {
  off: '关闭',
  subtle: '克制',
  playful: '活泼',
};

const VERBOSITY_LABEL: Record<string, string> = {
  minimal: '极简',
  concise: '精简',
  detailed: '详尽',
};

const MODE_LABEL: Record<string, string> = {
  casual: '日常',
  focus: '专注',
  incident: '事故',
  group: '群聊',
};

const AUDIENCE_LABEL: Record<string, string> = {
  private: '私聊',
  group: '群聊',
};


const AFFECT_LABEL: Record<string, string> = {
  neutral: '中性',
  positive: '积极',
  frustrated: '受挫',
  urgent: '紧急',
};

const DOMAIN_LABEL: Record<string, string> = {
  office: '办公',
  daily_life: '生活',
};

const FATIGUE_LABEL: Record<string, string> = {
  low: '精力充沛',
  normal: '常态',
  high: '明显疲劳',
};

/** 表达基线的一格：上标签、下读数。 */
const BaselineCell: React.FC<{ label: string; value: string }> = ({ label, value }) => (
  <div className="min-w-0">
    <Text tone="tertiary" size="xs" className="block">
      {label}
    </Text>
    <Text size="xs" className="block tabular-nums">
      {value}
    </Text>
  </div>
);


/** 时间收成「月-日 时:分」，界面不显示秒。 */
const pad = (value: number): string => String(value).padStart(2, '0');

const formatWhen = (ms: number): string => {
  const date = new Date(ms);
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
};

/** 把一条场景规则压成一句人话。 */
const describeRule = (rule: PersonaRule): string => {
  const scope = [
    rule.mode ? `${MODE_LABEL[rule.mode] ?? rule.mode}模式` : '',
    rule.audience ? AUDIENCE_LABEL[rule.audience] ?? rule.audience : '',
    rule.userAffect ? `${AFFECT_LABEL[rule.userAffect] ?? rule.userAffect}时` : '',
    rule.domain ? `${DOMAIN_LABEL[rule.domain] ?? rule.domain}场域` : '',
    rule.fatigueLevel ? `疲劳${FATIGUE_LABEL[rule.fatigueLevel] ?? rule.fatigueLevel}` : '',
  ]
    .filter(Boolean)
    .join(' · ');
  const voice = [
    rule.humor ? `幽默${HUMOR_LABEL[rule.humor] ?? rule.humor}` : '',
    rule.verbosity ? `篇幅${VERBOSITY_LABEL[rule.verbosity] ?? rule.verbosity}` : '',
    typeof rule.formality === 'number' ? `正式度 ${rule.formality}` : '',
    typeof rule.warmth === 'number' ? `温度 ${rule.warmth}` : '',
  ]
    .filter(Boolean)
    .join('，');
  return `${scope || '默认场景'}：${voice || '继承基线'}`;
};

export const PersonaStatusPanel: React.FC = () => {
  const [summary, setSummary] = useState<PersonaSummary | null>(null);
  const [portrait, setPortrait] = useState<PersonaPortrait | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    setLoading(true);
    // 形象与人格互不阻塞：没有形象也要能把人格数据读出来。
    void ipc
      .personaPortrait()
      .then(setPortrait)
      .catch(() => setPortrait(null));
    try {
      setSummary(await ipc.personaSummary());
      setError('');
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '读取人格失败');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const soul = summary?.soul;
  const voice = soul?.voice;
  const invariants = soul?.invariants ?? [];
  const rules = (soul?.rules ?? []) as PersonaRule[];
  const styleEntries = summary?.style.entries ?? [];

  return (
    <Panel clip>

      {error.length > 0 && (
        <div className="pb-3">
          <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={error} />
        </div>
      )}

      {loading && summary === null ? (
        <LoadingState label="正在读取人设" />
      ) : (
        <>
          <ListRow className="py-4 items-start gap-4">
            {portrait === null ? (
              <div className="flex h-28 w-28 shrink-0 items-center justify-center rounded-[12px] bg-surface-sunken">
                <Text tone="tertiary" size="xs">
                  未放置形象
                </Text>
              </div>
            ) : (
              <img
                alt="人格形象"
                src={`data:${portrait.mime};base64,${portrait.base64}`}
                className="h-28 w-28 shrink-0 rounded-[12px] object-cover"
              />
            )}
            <div className="flex-1 min-w-0 space-y-2">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <Text size="base" className="block truncate">
                    {soul?.identity?.name ?? '尚未设置人设'}
                  </Text>
                  <Text tone="secondary" size="xs" className="block mt-1">
                    {soul?.identity?.role ?? '在人设文件中补充定位后显示在这里'}
                  </Text>
                  {soul?.identity?.address && (
                    <Text tone="tertiary" size="xs" className="block mt-1">
                      {`称呼「${soul.identity.address}」`}
                    </Text>
                  )}
                </div>
                <IconButton label={loading ? '正在刷新人设' : '刷新人设'} disabled={loading} onClick={() => void load()}>
                  {loading ? <SpinnerIcon size={14} className="animate-spin" /> : <RetryIcon size={14} />}
                </IconButton>
              </div>
              {(soul?.traits ?? []).length > 0 && (
                <div className="flex flex-wrap gap-1.5">
                  {(soul?.traits ?? []).map((trait) => (
                    <Chip key={`trait-${trait}`}>{trait}</Chip>
                  ))}
                </div>
              )}
            </div>
          </ListRow>

          <ListRow className="py-4 items-start">
            <div className="flex-1 min-w-0 space-y-2">
              <Text strong>表达基线</Text>
              {voice === undefined ? (
                <EmptyHint>人设尚未提供表达基线</EmptyHint>
              ) : (
                <div className="grid max-w-[420px] grid-cols-2 gap-x-6 gap-y-2">
                  <BaselineCell label="温度" value={String(voice.warmth)} />
                  <BaselineCell label="正式度" value={String(voice.formality)} />
                  <BaselineCell label="幽默" value={HUMOR_LABEL[voice.humor] ?? voice.humor} />
                  <BaselineCell label="篇幅" value={VERBOSITY_LABEL[voice.verbosity] ?? voice.verbosity} />
                </div>
              )}
            </div>
          </ListRow>

          <VoicePresetSettings />

          <ListRow className="py-4 items-start">
            <div className="flex-1 min-w-0 space-y-1.5">
              <Text strong>不可违背项</Text>
              {invariants.length === 0 ? (
                <EmptyHint>人设尚未声明不可违背项</EmptyHint>
              ) : (
                invariants.map((invariant) => (
                  <div key={`invariant-${invariant}`} className="flex gap-2">
                    <Text tone="tertiary" size="xs" className="shrink-0">
                      ·
                    </Text>
                    <Text size="xs" className="min-w-0">
                      {invariant}
                    </Text>
                  </div>
                ))
              )}
            </div>
          </ListRow>

          {rules.length > 0 && (
            <ListRow className="py-4 items-start">
              <div className="flex-1 min-w-0 space-y-1.5">
                <Text strong>场景规则</Text>
                {rules.map((rule, index) => (
                  <Text key={`rule-${index}`} size="xs" className="block">
                    {describeRule(rule)}
                  </Text>
                ))}
              </div>
            </ListRow>
          )}

          <ListRow className="py-4 items-start">
            <div className="flex-1 min-w-0 space-y-1.5">
              <Text strong>习得风格</Text>
              {styleEntries.length === 0 ? (
                <EmptyHint>还没有已发布的习得风格</EmptyHint>
              ) : (
                styleEntries.map((entry) => (
                  <div key={entry.styleId} className="flex items-baseline justify-between gap-3">
                    <Text size="xs" className="min-w-0">
                      {entry.text}
                    </Text>
                    {entry.publishedAtMs !== null && (
                      <Text tone="secondary" size="xs" mono className="shrink-0 tabular-nums">
                        {formatWhen(entry.publishedAtMs)}
                      </Text>
                    )}
                  </div>
                ))
              )}
            </div>
          </ListRow>
        </>
      )}
    </Panel>
  );
};