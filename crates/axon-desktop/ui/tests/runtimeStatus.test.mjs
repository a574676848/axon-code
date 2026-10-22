import test from 'node:test';
import assert from 'node:assert/strict';
import { daemonHealthView, daemonStateView } from '../src/components/settings/runtimeStatus.ts';

/*
 * 后台运行状态的读法：结论四态、认不出的标识、租约失效与快照读不到。
 * 这些都是「出问题时才看得见」的分支，靠界面手点是验不到的。
 */

test('四态结论各有明确的点色与是否需要人处理', () => {
  assert.deepEqual(daemonStateView('running'), { label: '运行中', dot: 'done', attention: false });
  assert.deepEqual(daemonStateView('workers_stopped'), {
    label: '后台已停机',
    dot: 'warning',
    attention: true,
  });
  assert.deepEqual(daemonStateView('degraded'), { label: '运行受限', dot: 'warning', attention: true });
  assert.deepEqual(daemonStateView('stopped'), { label: '未运行', dot: 'error', attention: true });
});

test('认不出的结论标识原样置灰，绝不冒充运行中', () => {
  assert.deepEqual(daemonStateView('reindexing'), {
    label: 'reindexing',
    dot: 'idle',
    attention: false,
  });
  assert.deepEqual(daemonStateView(null), { label: '正在查询', dot: 'idle', attention: false });
  assert.deepEqual(daemonStateView(''), { label: '正在查询', dot: 'idle', attention: false });
});

test('租约有效时摊开排队读数，不报警', () => {
  const view = daemonHealthView(
    {
      host_id: 'axon-daemon-29296',
      lease_remaining_ms: 21_300,
      interactions_pending: 0,
      interactions_processing: 1,
      deliveries_pending: 2,
    },
    '',
  );
  assert.equal(view.warning, false);
  assert.equal(view.text, '后台租约剩余 22 秒 · 待领取 0 · 处理中 1 · 待投递 2');
});

test('租约失效是最需要人处理的一种读数：点明后果并给出恢复动作', () => {
  const view = daemonHealthView(
    {
      host_id: 'axon-daemon-18600',
      lease_remaining_ms: 0,
      interactions_pending: 1,
      interactions_processing: 0,
      deliveries_pending: 0,
    },
    '',
  );
  assert.equal(view.warning, true);
  assert.match(view.text, /租约已失效/);
  assert.match(view.text, /待领取 1/);
  assert.match(view.text, /重启/);
});

test('续租窗口超过一分半时读数换成分钟', () => {
  const view = daemonHealthView(
    {
      host_id: '',
      lease_remaining_ms: 600_000,
      interactions_pending: 0,
      interactions_processing: 0,
      deliveries_pending: 0,
    },
    '',
  );
  assert.match(view.text, /后台租约剩余 10 分钟/);
});

test('读不到快照时说明原因，不被当成一切正常', () => {
  const view = daemonHealthView(null, ' 无法连接 Axon Daemon（IPC 端点 axon-daemon） ');
  assert.equal(view.warning, false);
  assert.equal(view.text, '后台读数读不到：无法连接 Axon Daemon（IPC 端点 axon-daemon）');
  assert.equal(
    daemonHealthView(undefined, '').text,
    '后台读数读不到，无法确认 Worker 是否在推进',
  );
});
