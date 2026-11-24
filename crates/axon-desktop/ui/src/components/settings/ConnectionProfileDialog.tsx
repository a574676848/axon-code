import React, { useEffect, useRef, useState } from 'react';
import { AlertCircleIcon, CheckIcon, PulseIcon, TrashIcon } from '../../icons';
import type {
  CapabilityView,
  ConnectionConclusion,
  ConnectionTestResult,
} from '../../connectionTypes';
import { saveProfile, testConnection } from '../../connectionState';
import { Button, Input } from '../ui/controls';
import { Dialog } from '../ui/modal';
import { Banner, Chip, FormField, Hairline, Panel, Text } from '../ui/surfaces';

/*
 * 档案表单：新增与编辑共用一份，首开引导第 3 步复用的也是这一份。
 *
 * 三道边界：
 * 1. 凭据只在这张表单里存活：输入框是密码型、不回显已有值、每次打开都从空开始；
 *    只有试连接成功之后才允许保存，保存失败立即把密钥清掉，不留下一次点得动的残留。
 *    凭据永远不进任何上层状态，因此关掉弹窗即随之丢弃。
 * 2. 结论按回执的 conclusion 分档，展示文案一律取回执里的 message，界面只补一个
 *    分类名（凭据不匹配、证书有问题、连不上、地址格式有问题、需要升级客户端），
 *    不自己编原因，也不把证书问题并进「连不上」。
 * 3. 表单校验就地给提示，不用系统弹窗；命令行本身失败走提示条，不谎报成功。
 */

/** 档案表单的草稿；凭据只在这里存活。 */
export interface ConnectionProfileDraft {
  label: string;
  address: string;
  apiKey: string;
}

const EMPTY_DRAFT: ConnectionProfileDraft = { label: '', address: '', apiKey: '' };

/** 一类结论的界面说法；具体原因仍原样取回执的 message。 */
export const CONCLUSION_TEXT: Record<ConnectionConclusion, string> = {
  ok: '连接成功',
  credential: '凭据不匹配',
  certificate: '证书有问题',
  unreachable: '连不上',
  address: '地址格式有问题',
  protocol: '需要升级客户端',
};

/** 证书结论的界面说法。 */
export const CERTIFICATE_TEXT: Record<ConnectionTestResult['certificate'], string> = {
  trusted: '证书受信',
  invalid: '证书有误',
  unknown: '未校验证书',
};

/**
 * 一条只读读数：左标签、右取值。
 *
 * 连接分区与档案表单的结论区共用这一种排版，避免同一个「标签在上、取值在下」
 * 的读数在两处各写一遍。
 */
export const ConnectionFactRow: React.FC<{ label: string; value: React.ReactNode }> = ({
  label,
  value,
}) => (
  <div className="flex items-start gap-3">
    <Text tone="secondary" size="xs" className="w-24 shrink-0">
      {label}
    </Text>
    <div className="flex-1 min-w-0">{value}</div>
  </div>
);

/**
 * 能力摘要：只列覆盖后为真的项。
 *
 * 「已知且为假」按合同隐藏（那是后端的确定事实，留着只会误导）；「尚未取到」一律
 * 显示「未知」，不显示 0、也不把自己猜出来的一组能力画成结论。
 */
export const CapabilitySummary: React.FC<{ capabilities: CapabilityView[]; known: boolean }> = ({
  capabilities,
  known,
}) => {
  if (!known) {
    return (
      <Text tone="tertiary" size="xs">
        未知
      </Text>
    );
  }
  const enabled = capabilities.filter((item) => item.enabled && !item.hidden);
  if (enabled.length === 0) {
    return (
      <Text tone="tertiary" size="xs">
        后端未自报任何能力
      </Text>
    );
  }
  return (
    <div className="flex flex-wrap gap-1.5">
      {enabled.map((item) => (
        <Chip key={item.key}>{item.label}</Chip>
      ))}
    </div>
  );
};

/** 一次试连接的结论条：分类名做标题，原因原样取回执的 message。 */
export const ConnectionTestBanner: React.FC<{ result: ConnectionTestResult }> = ({ result }) =>
  result.conclusion === 'ok' ? (
    <Banner tone="success" icon={<CheckIcon size={13} />} title={result.message} />
  ) : (
    <Banner
      tone="danger"
      icon={<AlertCircleIcon size={13} />}
      title={CONCLUSION_TEXT[result.conclusion]}
      action={
        <Text tone="secondary" size="xs" className="block">
          {result.message}
        </Text>
      }
    />
  );

/** 一次成功试连接的读数：地址、版本、延迟、证书结论与能力摘要。 */
export const ConnectionTestReadings: React.FC<{ result: ConnectionTestResult }> = ({ result }) => (
  <Panel className="p-3 space-y-2">
    <ConnectionFactRow
      label="后端地址"
      value={
        <Text size="xs" mono truncate title={result.endpoint ?? ''}>
          {result.endpoint ?? '未知'}
        </Text>
      }
    />
    <ConnectionFactRow
      label="后端版本"
      value={<Text size="xs">{result.backendVersion ?? '未知'}</Text>}
    />
    <ConnectionFactRow
      label="往返延迟"
      value={
        <Text size="xs" className="tabular-nums">
          {result.latencyMs === null ? '未知' : `${result.latencyMs} 毫秒`}
        </Text>
      }
    />
    <ConnectionFactRow
      label="证书结论"
      value={<Text size="xs">{CERTIFICATE_TEXT[result.certificate]}</Text>}
    />
    <ConnectionFactRow
      label="能力摘要"
      value={<CapabilitySummary capabilities={result.capabilities} known />}
    />
  </Panel>
);

export interface ConnectionProfileDialogProps {
  /** 是否打开。 */
  open: boolean;
  /** 弹窗标题。 */
  title: string;
  /** 打开时的初值；凭据字段永远从空开始，因此这里给 apiKey 也不会被回显。 */
  initial?: Partial<ConnectionProfileDraft>;
  /** 保存按钮文案。 */
  submitLabel: string;
  /** 关闭。 */
  onClose: () => void;
  /** 成功（试连接通过并已保存）后调用。 */
  onSaved: () => void;
  /** 只试连接不保存时调用，供引导页展示结论。 */
  onTested?: (result: ConnectionTestResult) => void;
  /** 只读查看的原因；给了就锁住字段与保存动作（本机档案的地址由后端配置决定）。 */
  readOnlyReason?: string;
  /** 切换到此档案；不传则不渲染切换入口。 */
  onSwitch?: () => Promise<void>;
  /** 不能切换的原因；给了就禁用切换并就地说明。 */
  switchBlockedReason?: string;
  /** 删除此档案；不传则不渲染删除入口。 */
  onDelete?: () => Promise<void>;
  /** 不能删除的原因；给了就禁用删除并就地说明。 */
  deleteBlockedReason?: string;
}

export const ConnectionProfileDialog: React.FC<ConnectionProfileDialogProps> = ({
  open,
  title,
  initial,
  submitLabel,
  onClose,
  onSaved,
  onTested,
  readOnlyReason,
  onSwitch,
  switchBlockedReason,
  onDelete,
  deleteBlockedReason,
}) => {
  const readOnly = readOnlyReason !== undefined;
  const initialLabel = initial?.label ?? '';
  const initialAddress = initial?.address ?? '';
  const [draft, setDraft] = useState<ConnectionProfileDraft>(EMPTY_DRAFT);
  /** 最近一次试连接的回执；输入变化即作废，避免拿旧结论保存新地址。 */
  const [result, setResult] = useState<ConnectionTestResult | null>(null);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  /** 字段级校验：就地显示在字段下面。 */
  const [fieldError, setFieldError] = useState<string | null>(null);
  /** 命令本身没跑通的原因（与「结论是连不上」是两回事）。 */
  const [failure, setFailure] = useState<string | null>(null);
  /** 档案级动作（切换、删除）的失败原因。 */
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionBusy, setActionBusy] = useState<'switch' | 'delete' | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    // 每次打开都从空凭据开始：凭据从不回显，也从不跨一次打开存活。
    setDraft({ label: initialLabel, address: initialAddress, apiKey: '' });
    setResult(null);
    setTesting(false);
    setSaving(false);
    setFieldError(null);
    setFailure(null);
    setActionError(null);
    setActionBusy(null);
    setConfirmingDelete(false);
  }, [open, initialLabel, initialAddress]);

  const messageOf = (failure: unknown, fallback: string): string =>
    failure instanceof Error ? failure.message : fallback;

  /** 输入一改，上一次的结论就不再对应当前输入：作废并解除保存闸门。 */
  const edit = (patch: Partial<ConnectionProfileDraft>) => {
    setDraft((prev) => ({ ...prev, ...patch }));
    setResult(null);
    setFailure(null);
    setFieldError(null);
    setActionError(null);
  };

  const runTest = async () => {
    if (readOnly) return;
    const label = draft.label.trim();
    if (label.length === 0) {
      setFieldError('请填写档案名称');
      return;
    }
    setFieldError(null);
    setFailure(null);
    setActionError(null);
    setTesting(true);
    try {
      const next = await testConnection({ address: draft.address, apiKey: draft.apiKey });
      setResult(next);
      onTested?.(next);
    } catch (caught: unknown) {
      setResult(null);
      setFailure(messageOf(caught, '试连接失败'));
    } finally {
      setTesting(false);
    }
  };

  const runSave = async () => {
    if (readOnly || result === null || result.conclusion !== 'ok') return;
    setSaving(true);
    setFailure(null);
    try {
      await saveProfile(draft.label.trim(), draft.address);
      if (!alive.current) return;
      setDraft(EMPTY_DRAFT);
      onSaved();
    } catch (caught: unknown) {
      if (!alive.current) return;
      /*
       * 保存失败时后端已经把一次性凭据取走，再点一次也只会得到「请先测试连接」。
       * 因此这里把密钥与旧结论一并清掉，让人重新输入、重新实测，而不是留一个
       * 看起来还能点的保存按钮。
       */
      setDraft((prev) => ({ ...prev, apiKey: '' }));
      setResult(null);
      setFailure(`${messageOf(caught, '保存档案失败')}。请重新填写 API Key 并再次测试连接。`);
    } finally {
      setSaving(false);
    }
  };

  const runAction = async (kind: 'switch' | 'delete') => {
    const handler = kind === 'switch' ? onSwitch : onDelete;
    if (handler === undefined) return;
    setActionBusy(kind);
    setActionError(null);
    try {
      await handler();
      if (alive.current) onClose();
    } catch (caught: unknown) {
      if (!alive.current) return;
      setActionError(messageOf(caught, kind === 'switch' ? '切换档案失败' : '删除档案失败'));
    } finally {
      if (alive.current) setActionBusy(null);
    }
  };

  /*
   * 保存闸门：HTTP 与事件流**两条链路都要通过**。只按 HTTP 结论放行，会在
   * 「只对外开了 HTTP 端口」时留下一份配置能连、事件收不到的档案。
   */
  const canSave =
    !readOnly &&
    result !== null &&
    result.conclusion === 'ok' &&
    result.wsConclusion === 'ok';
  const switchBlocked = switchBlockedReason !== undefined || onSwitch === undefined;
  const deleteBlocked = deleteBlockedReason !== undefined || onDelete === undefined;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={title}
      size="md"
      hint={readOnly ? undefined : '先用页面上的密钥实测一次，通过后才能保存'}
      primary={
        readOnly
          ? undefined
          : {
              label: submitLabel,
              busyLabel: '保存中',
              icon: <CheckIcon size={13} />,
              busy: saving,
              disabled: !canSave,
              onClick: () => void runSave(),
            }
      }
      secondary={
        readOnly
          ? undefined
          : {
              label: '测试连接',
              busyLabel: '正在测试',
              icon: <PulseIcon size={13} />,
              busy: testing,
              onClick: () => void runTest(),
            }
      }
    >
      <FormField label="档案名称">
        <Input
          value={draft.label}
          placeholder="例如：公司云电脑"
          disabled={readOnly}
          onChange={(event) => edit({ label: event.target.value })}
        />
      </FormField>
      {fieldError !== null && (
        <Text tone="danger" size="xs" className="block">
          {fieldError}
        </Text>
      )}

      <FormField label="后端地址">
        <Input
          value={draft.address}
          placeholder="https://axon.example.com"
          disabled={readOnly}
          onChange={(event) => edit({ address: event.target.value })}
        />
      </FormField>
      {readOnly && <Text tone="secondary" size="xs" className="block">{readOnlyReason}</Text>}

      {!readOnly && (
        <FormField label="API Key">
          <Input
            type="password"
            autoComplete="off"
            value={draft.apiKey}
            placeholder="粘贴访问密钥，保存前必须实测通过"
            onChange={(event) => edit({ apiKey: event.target.value })}
          />
        </FormField>
      )}

      {result !== null && (
        <div className="space-y-2">
          <ConnectionTestBanner result={result} />

          {result.conclusion === 'ok' && <ConnectionTestReadings result={result} />}

          {result.plaintextPrivate && (
            <Banner
              tone="danger"
              icon={<AlertCircleIcon size={13} />}
              title="这个地址使用明文传输，凭据将以明文过网。"
            />
          )}
        </div>
      )}

      {failure !== null && (
        <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={failure} />
      )}

      {(onSwitch !== undefined ||
        switchBlockedReason !== undefined ||
        onDelete !== undefined ||
        deleteBlockedReason !== undefined) && (
        <>
          <Hairline />
          <div className="space-y-2">
            {(onSwitch !== undefined || switchBlockedReason !== undefined) && (
              <div className="flex items-center justify-between gap-3">
                <Text tone="secondary" size="xs" className="min-w-0">
                  {switchBlockedReason ?? '切换档案会重启应用，切换前请先保存手上的输入'}
                </Text>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={switchBlocked || actionBusy !== null}
                  onClick={() => void runAction('switch')}
                >
                  {actionBusy === 'switch' ? '正在切换' : '切换到此档案'}
                </Button>
              </div>
            )}

            {(onDelete !== undefined || deleteBlockedReason !== undefined) && (
              <div className="flex items-center justify-between gap-3">
                <Text tone="secondary" size="xs" className="min-w-0">
                  {deleteBlockedReason ??
                    (confirmingDelete
                      ? '删除后不可恢复，凭据库里的这把 Key 也会一并删掉'
                      : '删除档案时同步删除凭据库里的凭据')}
                </Text>
                <div className="shrink-0 flex items-center gap-1.5">
                  {confirmingDelete && (
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={actionBusy !== null}
                      onClick={() => setConfirmingDelete(false)}
                    >
                      取消
                    </Button>
                  )}
                  <Button
                    variant="danger"
                    size="sm"
                    disabled={deleteBlocked || actionBusy !== null}
                    onClick={() => {
                      if (deleteBlocked) return;
                      if (!confirmingDelete) {
                        setConfirmingDelete(true);
                        return;
                      }
                      void runAction('delete');
                    }}
                  >
                    <TrashIcon size={13} />
                    {confirmingDelete ? '确认删除' : '删除档案'}
                  </Button>
                </div>
              </div>
            )}
          </div>
        </>
      )}

      {actionError !== null && (
        <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={actionError} />
      )}
    </Dialog>
  );
};
