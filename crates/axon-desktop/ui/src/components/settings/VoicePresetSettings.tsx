import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, CheckIcon, RetryIcon } from '../../icons';
import * as ipc from '../../ipc';
import {
  FOLLOW_PERSONA_ID,
  parseVoicePresets,
  useVoicePresetId,
  voicePreferences,
  voicePresetRows,
  type VoicePreset,
  type VoicePresetRow,
} from '../../speech';
import { Button } from '../ui/controls';
import { Chip, EmptyState, ListRow, LoadingState, Text } from '../ui/surfaces';

/*
 * 音色：朗读时用哪一副嗓子。
 *
 * 清单来自网关的 `voice_presets`，第一项固定是「跟随人格」：选中它时不带预设 id，
 * 由后端按人格融合挑音色；选中预设则把 id 原样交给合成接口。选择落在本地存储
 * （键 `axon.voice.preset`）。
 *
 * 它落在人设页而不是系统设置：音色是人格的一部分，不是桌面外壳的偏好。
 *
 * 可用性仍以网关为准：条目带 `available: false` 时禁用并标注原因，只是不再提供
 * 安装入口（模型由发布侧随语音运行时提供，界面不负责下载）。
 */

/** 预设分区的三档状态。 */
export type VoicePresetStatus = 'loading' | 'ready' | 'failed';

/** 选中的圆点：选中是实心主色加对勾，未选中是空心描边。 */
const RadioMark: React.FC<{ checked: boolean }> = ({ checked }) => (
  <span
    aria-hidden="true"
    className={`grid h-3.5 w-3.5 shrink-0 place-items-center rounded-full ${
      checked ? 'bg-primary text-white' : 'border border-border-strong'
    }`}
  >
    {checked && <CheckIcon size={8} />}
  </span>
);

/** 行外壳：描边、圆角、悬浮与选中态，可选项与停用项共用同一套几何。 */
function rowShell(row: VoicePresetRow): string {
  const base = 'flex w-full items-center gap-2 rounded-md border px-3 py-2 transition-colors duration-100';
  if (row.checked) return `${base} border-primary bg-hover/60 shadow-lift`;
  return `${base} border-border`;
}

/** 行内方向键：上下一项或首末项，可选项之间循环。 */
type RowNavigate = (offset: number | 'first' | 'last') => void;

interface PresetRowProps {
  row: VoicePresetRow;
  /** 注册焦点句柄，方向键换项时把焦点一起带过去。 */
  register: (element: HTMLButtonElement | null) => void;
  onNavigate: RowNavigate;
  onSelect: (id: string) => void;
}

const PresetRow: React.FC<PresetRowProps> = ({ row, register, onNavigate, onSelect }) => (
  <button
    ref={register}
    type="button"
    role="radio"
    aria-checked={row.checked}
    aria-disabled={row.available ? undefined : true}
    tabIndex={row.checked ? 0 : -1}
    disabled={!row.available}
    onClick={() => onSelect(row.id)}
    onKeyDown={(event) => {
      if (event.key === 'ArrowDown' || event.key === 'ArrowRight') {
        event.preventDefault();
        onNavigate(1);
        return;
      }
      if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') {
        event.preventDefault();
        onNavigate(-1);
        return;
      }
      if (event.key === 'Home') {
        event.preventDefault();
        onNavigate('first');
        return;
      }
      if (event.key === 'End') {
        event.preventDefault();
        onNavigate('last');
      }
    }}
    className={`${rowShell(row)} text-left focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus ${
      row.available ? 'cursor-pointer hover:border-border-strong' : 'cursor-not-allowed'
    }`}
  >
    <RadioMark checked={row.checked} />
    <Text
      size="sm"
      tone={row.available ? 'primary' : 'tertiary'}
      strong={row.checked}
      truncate
      className="min-w-0 flex-1"
    >
      {row.name}
    </Text>
    {!row.available && <Chip>当前不可用</Chip>}
  </button>
);

export interface VoicePresetChoicesProps {
  /** 当前状态：加载中、就绪或失败。 */
  status: VoicePresetStatus;
  /** 已校验的网关清单；加载中与失败时为空数组。 */
  presets: readonly VoicePreset[];
  /** 清单失败时的真实原因，原样展示，不改写成成功口径。 */
  message: string;
  /** 存下来的预设 id；空串表示跟随人格。 */
  selectedId: string;
  /** 选中一项；停用项不会触发。 */
  onSelect: (id: string) => void;
  /** 清单失败后重试。 */
  onRetry: () => void;
}

/** 一列单选项：就绪时渲染整组，加载与失败各有自己的形态。 */
export const VoicePresetChoices: React.FC<VoicePresetChoicesProps> = ({
  status,
  presets,
  message,
  selectedId,
  onSelect,
  onRetry,
}) => {
  const rows = voicePresetRows(presets, selectedId);
  const refs = React.useRef<Array<HTMLButtonElement | null>>([]);
  /** 可选中项的下标；停用项不参与方向键循环。 */
  const selectable = rows.flatMap((row, index) => (row.available ? [index] : []));

  if (status === 'loading') {
    return <LoadingState label="正在读取音色预设" className="max-w-[420px]" />;
  }

  if (status === 'failed') {
    return (
      <EmptyState
        icon={<AlertCircleIcon size={16} />}
        title="音色预设暂时读不到"
        description={message}
        className="max-w-[420px]"
        action={
          <Button variant="outline" size="sm" onClick={onRetry}>
            <RetryIcon size={13} />
            重试
          </Button>
        }
      />
    );
  }

  /** 移到另一项并把选择带过去：单选组里焦点与选中永远是同一条。 */
  const navigate = (from: number, offset: number | 'first' | 'last'): void => {
    if (selectable.length === 0) return;
    const position = selectable.indexOf(from);
    const next =
      offset === 'first'
        ? selectable[0]
        : offset === 'last'
          ? selectable[selectable.length - 1]
          : selectable[(Math.max(position, 0) + offset + selectable.length) % selectable.length];
    refs.current[next]?.focus();
    onSelect(rows[next].id);
  };

  return (
    <div role="radiogroup" aria-label="音色" className="max-w-[420px] space-y-1">
      {rows.map((row, index) => (
        <PresetRow
          key={row.id}
          row={row}
          register={(element) => {
            refs.current[index] = element;
          }}
          onNavigate={(offset) => navigate(index, offset)}
          onSelect={onSelect}
        />
      ))}
    </div>
  );
};

/**
 * 音色分区：清单读取、失败重试与选择的落盘都在这里。
 *
 * 存下来的预设若已不在可用清单里（后端换了预设表、模型没随运行时提供），
 * 这里把它落回「跟随人格」：界面不能显示一个发不出去的音色。
 */
export const VoicePresetSettings: React.FC = () => {
  const selectedId = useVoicePresetId();
  const [status, setStatus] = useState<VoicePresetStatus>('loading');
  const [presets, setPresets] = useState<VoicePreset[]>([]);
  const [message, setMessage] = useState('');

  const load = useCallback(async () => {
    setStatus('loading');
    try {
      const parsed = parseVoicePresets(await ipc.voicePresets());
      setPresets(parsed);
      setStatus('ready');
      setMessage('');
      const stored = voicePreferences.presetId();
      const stillUsable = parsed.some(
        (preset) => preset.id === stored && preset.available,
      );
      if (stored !== FOLLOW_PERSONA_ID && !stillUsable) {
        voicePreferences.setPresetId(FOLLOW_PERSONA_ID);
      }
    } catch (failure) {
      setMessage(failure instanceof Error ? failure.message : '读取音色预设失败');
      setStatus('failed');
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <ListRow className="py-4 items-start">
      <div className="flex-1 min-w-0 space-y-2">
        <Text strong>音色</Text>
        <Text tone="secondary" size="xs" className="block">
          跟随人格时由后端按人格融合挑音色；也可以固定一个预设，之后每段朗读都用它。
        </Text>
        <VoicePresetChoices
          status={status}
          presets={presets}
          message={message}
          selectedId={selectedId}
          onSelect={(id) => voicePreferences.setPresetId(id)}
          onRetry={() => void load()}
        />
      </div>
    </ListRow>
  );
};
