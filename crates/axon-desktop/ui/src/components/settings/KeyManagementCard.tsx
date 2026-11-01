import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, CheckIcon, KeyIcon, PlusIcon, TrashIcon } from '../../icons';
import type { KeyIssueReceipt, KeyView } from '../../connectionTypes';
import * as ipc from '../../ipc';
import { Button, Input } from '../ui/controls';
import { Dialog } from '../ui/modal';
import {
  Banner,
  EmptyState,
  FormField,
  ListRow,
  LoadingState,
  Panel,
  SectionHeading,
  Text,
} from '../ui/surfaces';
import { formatDateTime } from '../pageFormat';

/*
 * 密钥管理卡片：为其他设备签发独立凭据，吊销即时生效。
 *
 * 三个边界不可退让：
 * 1. 只有超管凭据才渲染整块。普通 Key 的凭据本身有效、缺的是管理面权限，因此由调用方
 *    按 keyManagementVisible 整块不渲染，既不置灰也不留占位。
 * 2. 明文只在签发回执里显示一次。它不进任何全局状态，关掉回执弹窗即丢弃；列表只读
 *    标签、两个时间与前 6 位指纹。
 * 3. 吊销带二次确认。拿到一把 Key 的代价是要能登录这台后端，误删一把正在用的凭据
 *    会让那台设备立刻 401，因此一次点击不足以完成吊销。
 */

/** 从未用于鉴权时的读数：说清「没有使用过」，不显示 0 或伪造时刻。 */
const NEVER_USED = '从未使用';

/** 一次后端回执的原因；非 Error 时用兜底文案。 */
const messageOf = (failure: unknown, fallback: string): string =>
  failure instanceof Error ? failure.message : fallback;

export const KeyManagementCard: React.FC = () => {
  /** 已加载的清单；null 表示尚未读到。 */
  const [keys, setKeys] = useState<KeyView[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  /** 签发弹窗。 */
  const [issueOpen, setIssueOpen] = useState(false);
  const [label, setLabel] = useState('');
  const [issuing, setIssuing] = useState(false);
  const [issueError, setIssueError] = useState<string | null>(null);
  /** 签发回执：明文只在这里出现一次。 */
  const [receipt, setReceipt] = useState<KeyIssueReceipt | null>(null);
  /** 正在二次确认吊销的那一把。 */
  const [confirmingId, setConfirmingId] = useState<string | null>(null);
  const [revokingId, setRevokingId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      setKeys(await ipc.connectionKeysList());
    } catch (caught: unknown) {
      setKeys(null);
      setLoadError(messageOf(caught, '读取密钥列表失败'));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const openIssue = () => {
    setLabel('');
    setIssueError(null);
    setIssueOpen(true);
  };

  const closeIssue = () => {
    setIssueOpen(false);
    setLabel('');
    setIssueError(null);
  };

  const submitIssue = async () => {
    const trimmed = label.trim();
    if (trimmed.length === 0) {
      setIssueError('请填写用途标签：标签用于在审计里认出这把 Key');
      return;
    }
    setIssuing(true);
    setIssueError(null);
    try {
      const issued = await ipc.connectionKeysCreate(trimmed);
      setIssueOpen(false);
      setLabel('');
      // 明文只在回执弹窗里出现一次：先把它交给回执，再刷新列表。
      setReceipt(issued);
      await load();
    } catch (caught: unknown) {
      setIssueError(messageOf(caught, '签发 Key 失败'));
    } finally {
      setIssuing(false);
    }
  };

  const revoke = async (id: string) => {
    setRevokingId(id);
    setActionError(null);
    try {
      await ipc.connectionKeysRevoke(id);
      setConfirmingId(null);
      await load();
    } catch (caught: unknown) {
      setActionError(messageOf(caught, '吊销 Key 失败'));
    } finally {
      setRevokingId(null);
    }
  };

  const list = keys ?? [];

  return (
    <div className="space-y-2">
      <SectionHeading
        title="密钥管理"
        description="为其他设备签发独立凭据，吊销即时生效"
        action={
          <Button variant="outline" size="sm" onClick={openIssue}>
            <PlusIcon size={13} />
            签发新 Key
          </Button>
        }
      />

      {actionError !== null && (
        <Banner
          tone="danger"
          icon={<AlertCircleIcon size={13} />}
          title={actionError}
          action={
            <Button variant="ghost" size="sm" onClick={() => setActionError(null)}>
              知道了
            </Button>
          }
        />
      )}

      <Panel clip>
        {loading && (
          <LoadingState label="正在读取密钥列表" />
        )}

        {!loading && loadError !== null && (
          <div className="p-3">
            <Banner
              tone="danger"
              icon={<AlertCircleIcon size={13} />}
              title={`读取密钥列表失败：${loadError}`}
              action={
                <Button variant="ghost" size="sm" onClick={() => void load()}>
                  重试
                </Button>
              }
            />
          </div>
        )}

        {!loading && loadError === null && list.length === 0 && (
          <EmptyState
            icon={<KeyIcon size={20} />}
            title="还没有签发任何 Key"
            description="给每台设备单独签发一把 Key，出问题时可以只吊销那一把，不影响其他设备。"
            action={
              <Button variant="outline" size="sm" onClick={openIssue}>
                <PlusIcon size={13} />
                签发新 Key
              </Button>
            }
          />
        )}

        {!loading &&
          loadError === null &&
          list.map((key) => (
            <ListRow key={key.id}>
              <div className="flex-1 min-w-0">
                <Text size="sm" truncate className="block">
                  {key.label}
                </Text>
                <Text tone="secondary" size="micro" className="block tabular-nums">
                  {`创建 ${formatDateTime(key.createdAtMs)} · ${
                    key.lastUsedAtMs === null
                      ? NEVER_USED
                      : `最后使用 ${formatDateTime(key.lastUsedAtMs)}`
                  }`}
                </Text>
              </div>
              <Text tone="tertiary" size="micro" mono title="指纹前 6 位">
                {key.fingerprint}
              </Text>
              <div className="shrink-0 flex items-center gap-1.5">
                {confirmingId === key.id && (
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={revokingId !== null}
                    onClick={() => setConfirmingId(null)}
                  >
                    取消
                  </Button>
                )}
                <Button
                  variant="danger"
                  size="sm"
                  disabled={revokingId !== null}
                  onClick={() => {
                    if (confirmingId !== key.id) {
                      setConfirmingId(key.id);
                      return;
                    }
                    void revoke(key.id);
                  }}
                >
                  <TrashIcon size={13} />
                  {revokingId === key.id
                    ? '正在吊销'
                    : confirmingId === key.id
                      ? '确认吊销'
                      : '吊销'}
                </Button>
              </div>
            </ListRow>
          ))}
      </Panel>

      {confirmingId !== null && (
        <Text tone="secondary" size="xs" className="block">
          吊销后那台设备会立刻 401，需要重新签发一把新 Key 才能继续连接。
        </Text>
      )}

      {/* ------------------------------------------------------------ 签发 */}
      <Dialog
        open={issueOpen}
        onClose={closeIssue}
        title="签发新 Key"
        size="sm"
        hint="签发后明文只显示一次"
        primary={{
          label: '签发',
          busyLabel: '正在签发',
          icon: <PlusIcon size={13} />,
          busy: issuing,
          onClick: () => void submitIssue(),
        }}
        secondary={{ label: '取消', onClick: closeIssue }}
      >
        <FormField label="用途标签">
          <Input
            value={label}
            placeholder="例如：公司笔电"
            autoComplete="off"
            onChange={(event) => {
              setLabel(event.target.value);
              setIssueError(null);
            }}
          />
        </FormField>
        {issueError !== null && (
          <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={issueError} />
        )}
      </Dialog>

      {/* ------------------------------------------------------------ 回执 */}
      <Dialog
        open={receipt !== null}
        onClose={() => setReceipt(null)}
        title="已签发新 Key"
        size="md"
        hint="关掉这一页就再也看不到明文了"
        primary={{
          label: '我已保存',
          icon: <CheckIcon size={13} />,
          onClick: () => setReceipt(null),
        }}
      >
        <Text tone="secondary" size="xs" className="block">
          {`「${receipt?.label ?? ''}」已经可以用了。把它填到那台设备的档案里：`}
        </Text>
        <div className="rounded-sm bg-surface-sunken px-3 py-2 select-all">
          <Text size="xs" mono className="block break-all">
            {receipt?.plaintext ?? ''}
          </Text>
        </div>
        <Banner
          tone="danger"
          icon={<AlertCircleIcon size={13} />}
          title="离开此页后无法再看：请立刻把它保存到安全位置，或直接填进那台设备。"
        />
      </Dialog>
    </div>
  );
};
