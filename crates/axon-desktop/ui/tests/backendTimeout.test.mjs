import test from 'node:test';
import assert from 'node:assert/strict';
import { call, readTimeoutFor, withTimeout } from '../src/backend.ts';

/*
 * 读命令的读取超时。
 *
 * 界面里每一条读命令都有自己的加载态：调用卡住不回来，那一页就永久转圈。
 * 这里锁住三件事：
 * 1. 哪些命令设了超时（只读），哪些没有（用户主动触发的长命令）；
 * 2. 超时之后调用方拿到的是错误态，而不是一直等待；
 * 3. 迟到的成功回包不会再写一次状态，把错误态盖掉。
 */

const REAL_FETCH = globalThis.fetch;
const HAD_WINDOW = 'window' in globalThis;
const REAL_WINDOW = globalThis.window;

function restoreGlobals() {
  globalThis.fetch = REAL_FETCH;
  if (HAD_WINDOW) globalThis.window = REAL_WINDOW;
  else delete globalThis.window;
}

/** 调试桥响应：结构照 `callRaw` 的解析口径，成功与失败两条路都用它。 */
function bridgeReply(data) {
  return { ok: true, status: 200, json: async () => ({ ok: true, data }) };
}

/** 让被 mock 的定时器以外的微任务跑完（setImmediate 不在 mock 的 apis 里）。 */
function flushMicrotasks() {
  return new Promise((resolve) => {
    setImmediate(resolve);
  });
}

test('只读命令有读取超时，用户主动触发的长命令没有超时', () => {
  for (const command of ['workspace_list', 'gateway_health', 'conversation_history', 'memory_list']) {
    assert.equal(readTimeoutFor(command), 20_000, `${command} 属于只读命令，应当有读取超时`);
  }
  for (const command of ['pick_directory', 'export_markdown', 'connection_test', 'pricing_sync']) {
    assert.equal(readTimeoutFor(command), null, `${command} 可能长时间等待，不得设超时`);
  }
  for (const command of ['session_set_pinned', 'conversation_submit', 'terminal_open']) {
    assert.equal(readTimeoutFor(command), null, `${command} 是写操作或用户驱动命令，不在超时表内`);
  }
});

test('超时包装器先定局，迟到的成功不会改写结论', async () => {
  let settleLate = null;
  const work = new Promise((resolve) => {
    settleLate = resolve;
  });

  await assert.rejects(withTimeout(work, 5), /读取超时：后台服务在 5 毫秒/);

  settleLate('late-success');
  await flushMicrotasks();
});

test('调试桥通道：读命令超时落到错误态，迟到的回包不覆盖它', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let settleLate = null;
  globalThis.fetch = async (url, init) => {
    const command = JSON.parse(init.body).cmd;
    // 健康复探走正常回包：这样一次超时不会被误判成「后端没起来」。
    if (command === 'gateway_health') return bridgeReply({ connected: true, available: true });
    return new Promise((resolve) => {
      settleLate = resolve;
    });
  };

  let state = 'loading';
  const reading = call('workspace_list').then(
    () => {
      state = 'ready';
    },
    () => {
      state = 'error';
    },
  );

  t.mock.timers.tick(20_000);
  await reading;
  assert.equal(state, 'error', '读命令超时后必须落到错误态，而不是停在加载态');

  assert.notEqual(settleLate, null, '被超时的那次调用仍在等回包');
  settleLate(bridgeReply([{ id: 'late' }]));
  await flushMicrotasks();
  assert.equal(state, 'error', '迟到的成功回包不得把错误态改回成功态');

  restoreGlobals();
});

test('宿主 invoke 通道：读命令同样超时，迟到的回包不覆盖错误态', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let settleLate = null;
  globalThis.window = {
    __TAURI_INTERNALS__: {
      invoke: () =>
        new Promise((resolve) => {
          settleLate = resolve;
        }),
    },
  };

  let state = 'loading';
  const reading = call('skills_list').then(
    () => {
      state = 'ready';
    },
    () => {
      state = 'error';
    },
  );

  t.mock.timers.tick(20_000);
  await reading;
  assert.equal(state, 'error', '宿主通道读命令超时后同样落到错误态');

  settleLate(['late']);
  await flushMicrotasks();
  assert.equal(state, 'error', '迟到的宿主回包不得覆盖错误态');

  restoreGlobals();
});

test('没有超时的命令不会被读取超时截断', async () => {
  const work = new Promise((resolve) => {
    setImmediate(() => resolve('slow-but-allowed'));
  });
  assert.equal(await withTimeout(work, 60_000), 'slow-but-allowed');
});
