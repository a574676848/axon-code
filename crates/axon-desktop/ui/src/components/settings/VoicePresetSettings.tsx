import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, RetryIcon, SpeakerIcon, SpinnerIcon, StopIcon } from '../../icons';
import * as ipc from '../../ipc';
import { createAudioPlayer } from '../../speech/audioPlayer';
import { requestVoicePreview } from '../../speech/client';
import {
  FOLLOW_PERSONA_ID,
  parseVoicePresets,
  useVoicePresetId,
  voicePreferences,
  voicePresetRows,
  type VoicePreset,
} from '../../speech';
import { Button, IconButton } from '../ui/controls';
import { EmptyState, ListRow, LoadingState, Text } from '../ui/surfaces';
import { Select, type SelectOption } from '../ui/overlays';
import { VoiceWave } from '../conversation/VoiceWave';

/*
 * 音色：朗读时用哪一副嗓子。
 *
 * 清单来自网关的 `voice_presets`，第一项固定是「跟随人格」：选中它时不带预设 id，
 * 由后端按人格融合挑音色；选中预设则把 id 原样交给合成接口。选择落在本地存储
 * （键 `axon.voice.preset`）。
 *
 * 它落在人设页而不是系统设置：音色是人格的一部分，不是桌面外壳的偏好。
 *
 * 形态是下拉而不是把五档平铺成五行：同一时刻只可能有一副嗓子生效，平铺会把
 * 一个单选问题画成一张选项表，把「我选了哪个」藏在一堆等宽行里。停用预设仍在
 * 下拉里列出并标注原因，只是不可选（模型由发布侧随语音运行时提供，界面不负责下载）。
 */

/** 预设分区的三档状态。 */
export type VoicePresetStatus = 'loading' | 'ready' | 'failed';

export interface VoicePresetSelectProps {
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
  /** 正在播放试听。 */
  previewing?: boolean;
  /** 正在等待试听音频。 */
  previewBusy?: boolean;
  /** 试听动作；不传时不渲染试听按钮。 */
  onPreview?: () => void;
  /** 当前选中项是否可用（停用预设不给试听）。 */
  previewDisabled?: boolean;
}

/** 音色下拉：一行控件加一枚试听按钮，停用项在下拉里标注原因。 */
export const VoicePresetSelect: React.FC<VoicePresetSelectProps> = ({
  status,
  presets,
  message,
  selectedId,
  onSelect,
  onRetry,
  previewing = false,
  previewBusy = false,
  onPreview,
  previewDisabled = false,
}) => {
  const rows = voicePresetRows(presets, selectedId);
  const options: SelectOption[] = rows.map((row) => ({
    value: row.id,
    label: row.name,
    disabled: !row.available,
    hint: row.available ? undefined : '当前不可用',
  }));
  const selected = rows.find((row) => row.checked) ?? rows[0];

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

  return (
    <div className="flex max-w-[420px] items-center gap-2">
      <Select
        label="音色"
        value={selected?.id ?? FOLLOW_PERSONA_ID}
        options={options}
        onChange={onSelect}
        className="min-w-0 flex-1"
      />
      {onPreview !== undefined && (
        <IconButton
          label={previewing ? '停止试听' : '试听当前音色'}
          disabled={previewBusy || previewDisabled || selected?.available === false}
          onClick={onPreview}
          className={previewing ? 'bg-active text-text-primary' : ''}
        >
          {previewBusy ? (
            <SpinnerIcon size={14} className="animate-spin" />
          ) : previewing ? (
            <span className="inline-flex items-center gap-1">
              <VoiceWave active size="xs" />
              <StopIcon size={14} />
            </span>
          ) : (
            <SpeakerIcon size={14} />
          )}
        </IconButton>
      )}
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
  /** 正在播放试听；成功发起后为真。 */
  const [previewing, setPreviewing] = useState(false);
  /** 正在等待试听音频。 */
  const [previewBusy, setPreviewBusy] = useState(false);
  /** 试听失败的中文结论；成功或换一项即清空。 */
  const [previewError, setPreviewError] = useState<string | null>(null);
  /** 当前播放器；同一时刻只保留一段试听音频。 */
  const playerRef = React.useRef<ReturnType<typeof createAudioPlayer> | null>(null);
  /** 请求代次：连点或换项时旧回执直接作废，不覆盖新的试听。 */
  const requestRef = React.useRef(0);

  const stopPreview = useCallback(() => {
    requestRef.current += 1;
    playerRef.current?.stop();
    playerRef.current = null;
    setPreviewing(false);
    setPreviewBusy(false);
  }, []);

  const togglePreview = useCallback(async () => {
    if (previewing) {
      stopPreview();
      return;
    }
    stopPreview();
    setPreviewError(null);
    const generation = requestRef.current;
    setPreviewBusy(true);
    try {
      const payload = await requestVoicePreview(
        selectedId === FOLLOW_PERSONA_ID || selectedId.length === 0 ? null : selectedId,
      );
      if (requestRef.current !== generation) return;
      const player = createAudioPlayer(payload, {
        onEnded: () => {
          if (requestRef.current !== generation) return;
          playerRef.current = null;
          setPreviewing(false);
        },
        onFailed: (failure) => {
          if (requestRef.current !== generation) return;
          playerRef.current = null;
          setPreviewing(false);
          setPreviewError(
            failure instanceof Error && failure.message.length > 0
              ? failure.message
              : '试听播放失败',
          );
        },
      });
      playerRef.current = player;
      setPreviewing(true);
      await player.play();
    } catch (failure) {
      if (requestRef.current !== generation) return;
      setPreviewError(failure instanceof Error ? failure.message : '试听失败，请稍后重试');
    } finally {
      if (requestRef.current === generation) setPreviewBusy(false);
    }
  }, [previewing, selectedId, stopPreview]);

  useEffect(
    () => () => {
      requestRef.current += 1;
      playerRef.current?.stop();
      playerRef.current = null;
    },
    [],
  );

  const load = useCallback(async () => {
    setStatus('loading');
    try {
      const parsed = parseVoicePresets(await ipc.voicePresets());
      setPresets(parsed);
      setStatus('ready');
      setMessage('');
      const stored = voicePreferences.presetId();
      const stillUsable = parsed.some((preset) => preset.id === stored && preset.available);
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

  const current = presets.find((preset) => preset.id === selectedId);

  return (
    <ListRow className="py-4 items-start">
      <div className="flex-1 min-w-0 space-y-2">
        <Text strong>音色</Text>
        <Text tone="secondary" size="xs" className="block">
          跟随人格时由后端按人格融合挑音色；也可以固定一个预设，之后每段朗读都用它。
        </Text>
        <VoicePresetSelect
          status={status}
          presets={presets}
          message={message}
          selectedId={selectedId}
          onSelect={(id) => {
            stopPreview();
            voicePreferences.setPresetId(id);
          }}
          onRetry={() => void load()}
          previewing={previewing}
          previewBusy={previewBusy}
          onPreview={() => void togglePreview()}
          previewDisabled={status !== 'ready' || current?.available === false}
        />
        {previewError !== null && (
          <div role="status" aria-live="polite" className="max-w-[420px] pt-1">
            <Text size="xs" tone="danger">
              {previewError}
            </Text>
          </div>
        )}
      </div>
    </ListRow>
  );
};
