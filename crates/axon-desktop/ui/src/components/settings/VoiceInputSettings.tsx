import React, { useCallback, useEffect, useRef, useState } from 'react';
import { SpeakerIcon, StopIcon } from '../../icons';
import {
  createLevelPreview,
  listInputDevices,
  selectedDeviceId,
  setSelectedDeviceId,
  SYSTEM_DEFAULT_DEVICE,
  type InputDevice,
  type LevelPreview,
} from '../../speech/devices';
import { readVoiceRuntime, type VoiceRuntime } from '../../speech/voicePermission';
import { IconButton } from '../ui/controls';
import { Text } from '../ui/surfaces';
import { Select, type SelectOption } from '../ui/overlays';
import { VoiceWave } from '../conversation/VoiceWave';

/*
 * 语音输入设备：用哪一路麦克风，以及「这一路现在有没有声音」。
 *
 * 与成熟语音输入产品同一条原则：设备是用户自己的选择，选过就记住；候选来自
 * 系统真实枚举，不编造列表。右侧的电平预览用 Web Audio 实时读数，按一下开始、
 * 再按一下停止；离开这一页会自动释放设备，不会在后台一直开着麦克风。
 *
 * 未授权时浏览器会把设备名擦掉，界面只说清「授权后才会显示名称」，
 * 不假装列出了一份完整清单。
 */

export const VoiceInputSettings: React.FC = () => {
  const [devices, setDevices] = useState<InputDevice[]>([
    { id: SYSTEM_DEFAULT_DEVICE, label: '系统默认麦克风' },
  ]);
  const [deviceId, setDeviceId] = useState<string>(() => selectedDeviceId());
  const [previewing, setPreviewing] = useState(false);
  /** 本地语音运行时的就绪结论；读不到时为 null。 */
  const [runtime, setRuntime] = useState<VoiceRuntime | null>(null);
  const [error, setError] = useState<string | null>(null);
  const previewRef = useRef<LevelPreview | null>(null);

  const refresh = useCallback(async () => {
    const list = await listInputDevices();
    setDevices(list);
    // 选过的设备若已不在列表里（拔掉了），如实回落到系统默认。
    setDeviceId((current) => (list.some((item) => item.id === current) ? current : SYSTEM_DEFAULT_DEVICE));
  }, []);

  useEffect(() => {
    void readVoiceRuntime().then(setRuntime);
    void refresh();
    const media = typeof navigator === 'undefined' ? undefined : navigator.mediaDevices;
    media?.addEventListener?.('devicechange', () => void refresh());
    return () => {
      media?.removeEventListener?.('devicechange', () => void refresh());
    };
  }, [refresh]);

  const stopPreview = useCallback(() => {
    previewRef.current?.stop();
    previewRef.current = null;
    setPreviewing(false);
  }, []);

  const togglePreview = useCallback(async () => {
    if (previewing) {
      stopPreview();
      return;
    }
    setError(null);
    const preview = createLevelPreview();
    previewRef.current = preview;
    setPreviewing(true);
    try {
      await preview.start(deviceId);
      // 授权之后设备名才可读，列一次让选择器跟上。
      void refresh();
    } catch (failure) {
      setPreviewing(false);
      previewRef.current = null;
      preview.stop();
      setError(failure instanceof Error ? failure.message : '无法打开这个麦克风');
    }
  }, [deviceId, previewing, refresh, stopPreview]);

  // 离开这一页即释放设备：预览不该在后台一直占着麦克风。
  useEffect(() => () => previewRef.current?.stop(), []);

  const options: SelectOption[] = devices.map((device) => ({
    value: device.id,
    label: device.label,
  }));

  return (
    <div className="px-3.5 py-3 hairline-b border-border-hairline">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 space-y-0.5">
          <Text size="sm">语音输入设备</Text>
          <Text tone="secondary" size="xs" className="block">
            选中哪一路就用哪一路，不会退回到别的麦克风；设备名在首次授权后才会显示。
          </Text>
          <Text size="xs" tone={runtime?.available === false ? 'danger' : 'tertiary'} className="block">
            {runtime === null
              ? '读取本地语音运行时状态中'
              : runtime.available
                ? '本地语音运行时已就绪'
                : '本地语音运行时未安装：重新运行 Axon 安装脚本补齐语音依赖'}
          </Text>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Select
            label="语音输入设备"
            value={deviceId}
            options={options}
            onChange={(next) => {
              stopPreview();
              setSelectedDeviceId(next);
              setDeviceId(next);
            }}
            className="w-[220px]"
          />
          <IconButton
            label={previewing ? '停止试听输入设备' : '试听输入设备'}
            onClick={() => void togglePreview()}
            className={previewing ? 'bg-active text-text-primary' : ''}
          >
            {previewing ? (
              <span className="inline-flex items-center gap-1">
                <VoiceWave active size="xs" />
                <StopIcon size={14} />
              </span>
            ) : (
              <SpeakerIcon size={14} />
            )}
          </IconButton>
        </div>
      </div>
      {error !== null && (
        <div role="status" aria-live="polite" className="pt-1.5">
          <Text size="xs" tone="danger">
            {error}
          </Text>
        </div>
      )}
    </div>
  );
};
