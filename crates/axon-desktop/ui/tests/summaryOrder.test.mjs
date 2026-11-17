import test from "node:test";
import assert from "node:assert/strict";
import { orderTurnCells } from "../src/conversation/transcript/order.ts";

/*
 * 生成过程中分割线就已经立起来（用量帧一到就有读数），此后到达的工具行与正文会排在
 * 它后面。这里固定：分割线必须跟在这一轮的全部工作之后，读起来才是「用户 → 过程 →
 * 正文 → 读数」，而不是把一句话截成两段。
 */

const cell = (id, role, extra = {}) => ({ id, role, atMs: null, ...extra });

test("后到的工作排在分割线之前，分割线始终收口", () => {
  const cells = [
    cell("u1", "user"),
    cell("r1", "reasoning", { done: true }),
    cell("s", "summary", { durationMs: 1200, usage: null, firstTokenAvgMs: null, generationTpsX10: null }),
    cell("t1", "tool", { toolName: "shell_command", status: "done" }),
    cell("a1", "assistant", { text: "结果", streaming: false }),
  ];
  const ordered = orderTurnCells([...cells]);
  assert.deepEqual(
    ordered.map((c) => c.id),
    ["u1", "r1", "t1", "a1", "s"],
    "分割线必须排在思考、工具与正文之后",
  );
});
