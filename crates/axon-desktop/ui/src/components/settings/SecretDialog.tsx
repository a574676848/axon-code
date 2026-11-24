import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, CheckIcon, TrashIcon } from '../../icons';
import * as ipc from '../../ipc';
import { Input } from '../ui/controls';
import { Dialog } from '../ui/overlays';
import { Banner, Chip, FormField, Text } from '../ui/surfaces';

/*
 * 密钥配置弹窗：把密钥写进配置相邻的 .env。
 * 明文只在输入框里出现一次，保存后既不回显也不回传；弹窗只展示
 * 「已配置 / 未配置」这一位状态，并支持清除。
 */

interface SecretDialogProps {
  /** 是否打开。 */
  open: boolean;
  /** 环境变量名，来自供应商或配置项。 */
  envKey: string;
  /** 关闭。 */
  onClose: () => void;
  /** 写入或清除成功后回调，供上层刷新状态。 */
  onChanged: () => void;
}

export const SecretDialog: React.FC<SecretDialogProps> = ({
  open,
  envKey,
  onClose,
  onChanged,
}) => {
  const [value, setValue] = useState('');
  const [configured, setConfigured] = useState(false);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (envKey.trim().length === 0) {
      setConfigured(false);
      return;
    }
    try {
      setConfigured(await ipc.secretStatus(envKey.trim()));
    } catch {
      setConfigured(false);
    }
  }, [envKey]);

  useEffect(() => {
    if (!open) return;
    setValue('');
    setNotice(null);
    setError(null);
    setBusy(false);
    void refresh();
  }, [open, refresh]);

  const save = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await ipc.secretSet(envKey.trim(), value);
      setValue('');
      await refresh();
      setNotice('已写入 .env，立即生效');
      onChanged();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '写入密钥失败');
    } finally {
      setBusy(false);
    }
  };

  const clear = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await ipc.secretClear(envKey.trim());
      await refresh();
      setNotice('已从 .env 中移除该密钥');
      onChanged();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '清除密钥失败');
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="配置密钥"
      size="sm"
      secondary={
        configured
          ? {
              label: '清除该密钥',
              icon: <TrashIcon size={13} />,
              danger: true,
              busy,
              onClick: () => void clear(),
            }
          : undefined
      }
      primary={{
        label: '写入密钥',
        busyLabel: '写入中',
        icon: <CheckIcon size={13} />,
        busy,
        disabled: value.trim().length === 0,
        onClick: () => void save(),
      }}
    >
      <div className="flex items-center gap-2">
        <Text mono truncate>
          {envKey.trim().length > 0 ? envKey : '未设置环境变量名'}
        </Text>
        <Chip>{configured ? '已配置' : '未配置'}</Chip>
      </div>

      <FormField label="密钥值">
        <Input
          type="password"
          autoFocus
          value={value}
          placeholder={configured ? '输入新值以覆盖现有密钥' : '粘贴密钥值'}
          onChange={(event) => {
            setValue(event.target.value);
            setNotice(null);
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && value.trim().length > 0) void save();
          }}
        />
      </FormField>

      {notice !== null && <Banner tone="success" icon={<CheckIcon size={13} />} title={notice} />}
      {error !== null && <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={error} />}

      <Text tone="tertiary" size="xs" className="block leading-relaxed">
        密钥写入配置相邻的 .env 文件，不进入 config.toml，也不会回传到界面。
        已有密钥只显示配置状态，值不会回显。
      </Text>
    </Dialog>
  );
};
