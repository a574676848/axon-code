import type { TranscriptCell } from '../../conversation/transcript/state';

/*
 * 时间线里的作业读取折叠。
 *
 * 模型常以小 yield 反复读同一个作业：每次都是一条独立调用，原样铺出来就是一堵
 * 一模一样的行（实测同一轮里能连出十几条，每条 1 秒）。它们读的是同一件事，
 * 因此保留最后一条（最新输出与结论），其余折成次数与合计耗时。
 *
 * 只并**相邻**的同类读取：中间隔着别的单元时说明那是两次独立的观察动作，不该并。
 */

/** 从 `job_output` 的调用参数里取作业标识；不是这个工具或参数认不出时返回 null。 */
export function jobOutputJobId(toolName: string, args: string): string | null {
  const name = toolName.toLowerCase();
  if (name.slice(name.lastIndexOf('.') + 1) !== 'job_output') return null;
  const text = args.trim();
  if (text.length === 0) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return null;
  }
  if (parsed === null || typeof parsed !== 'object') return null;
  const record = parsed as Record<string, unknown>;
  const raw = record.job_id ?? record.jobId;
  if (typeof raw === 'string' && raw.trim().length > 0) return raw.trim();
  if (typeof raw === 'number' && Number.isFinite(raw)) return String(raw);
  return null;
}

/** 一条渲染项：一个单元，外加它替掉了多少条同类作业读取。 */
export interface FoldedCell {
  cell: TranscriptCell;
  /** 含自己在内的读取条数；1 表示没有并。 */
  repeats: number;
  /** 这些读取的耗时合计；一条都取不到耗时时为空。 */
  repeatsMs: number | null;
}

/** 相邻、同作业的读取并成一条；其余单元原样保留。 */
export function foldJobReads(cells: readonly TranscriptCell[]): FoldedCell[] {
  const folded: FoldedCell[] = [];
  for (const cell of cells) {
    const jobId = jobIdOf(cell);
    const previous = folded[folded.length - 1];
    if (jobId !== null && previous !== undefined && jobIdOf(previous.cell) === jobId) {
      folded[folded.length - 1] = {
        cell,
        repeats: previous.repeats + 1,
        repeatsMs: sumMs(previous.repeatsMs, durationOf(cell)),
      };
      continue;
    }
    folded.push({ cell, repeats: 1, repeatsMs: durationOf(cell) });
  }
  return folded;
}

/** 这条单元若是一次作业读取，给出它读的作业标识。 */
function jobIdOf(cell: TranscriptCell): string | null {
  return cell.role === 'tool' ? jobOutputJobId(cell.toolName, cell.arguments) : null;
}

/** 工具单元的实测耗时；其余单元没有耗时。 */
function durationOf(cell: TranscriptCell): number | null {
  return cell.role === 'tool' ? cell.durationMs : null;
}

/** 合计耗时：两边都取不到时仍然是空，缺一侧按 0 累加。 */
function sumMs(left: number | null, right: number | null): number | null {
  if (left === null && right === null) return null;
  return (left ?? 0) + (right ?? 0);
}
