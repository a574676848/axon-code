/**
 * 右侧栏 tab 类型注册表。
 *
 * 右侧栏里能开什么，由一份静态声明决定，而不是写死在布局代码里：
 * 页类型（文件这类固定页面）按 kind 打开，资源类型给出地址模式，
 * 由「谁认领这个地址」决定用哪个类型打开。会话里的文件链接、工具行的
 * 行号引用都只负责给地址，认领与去重统一在这里裁决。
 */

/** 资源地址前缀：只认自己这一套，外部地址不会被当成本地文件。 */
const FILE_PREFIX = 'axon-resource://file/';

/** 改动对比地址前缀：一轮一个地址，文件下标走导航参数。 */
const CHANGES_PREFIX = 'axon-resource://changes/';

/** 打开资源时可带的导航参数。 */
export interface ResourceParams {
  /** 定位到第几行（从 1 开始）。 */
  line?: number;
  /** 落到第几个文件（从 0 开始）；改动对比页专用。 */
  index?: number;
  /** 终端要启动的 shell；缺省按配置顺序取第一个可用的。 */
  shellPath?: string;
}

/** 引导页入口的图形身份；图形本身活在引导页自己的模块里。 */
export type GuideArtworkId = 'files' | 'terminal';

/** 一个页类型在引导页上的入口。 */
export interface GuideEntry {
  /** 排序号，小的在前。 */
  order: number;
  /** 入口标题。 */
  title: string;
  /** 入口说明；引导页只列少数几个入口时显示。 */
  description: string;
  /** 图形身份。 */
  artwork: GuideArtworkId;
  /** 生效的快捷键，按书写顺序给出按键名；渲染时用 `+` 连接。 */
  shortcut: string[];
}

/** 一个 tab 类型。 */
export interface TabType {
  /** 类型身份，全局唯一。 */
  kind: string;
  /** 认领的资源地址模式；页类型不给出。 */
  patterns?: string[];
  /** 同一档内的优先级，数值大的先认领。 */
  priority?: number;
  /** 命中后的否决权。 */
  canOpen?: (address: string) => boolean;
  /** 认领档位：具体类型压过兜底类型。 */
  tier?: 'specific' | 'fallback';
  /** tab 标题；在打开时固定。 */
  title: (address: string | null) => string;
  /** 页类型能否多开；资源类型天然按地址去重，不看这一位。 */
  multiple?: boolean;
  /** 是否在引导页上给出入口；省略即不出现。 */
  guide?: GuideEntry;
}

/** 资源地址对应的文件名，作为预览 tab 的标题。 */
function baseName(address: string): string {
  const path = addressPath(address);
  if (path === null) return '预览';
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}

const REGISTERED: TabType[] = [
  {
    kind: 'guide',
    tier: 'specific',
    title: () => '开始',
  },
  {
    kind: 'files',
    tier: 'specific',
    title: () => '文件',
    guide: {
      order: 10,
      title: '工作区文件',
      description: '浏览会话工作区的文件',
      artwork: 'files',
      shortcut: ['Ctrl', 'Alt', 'P'],
    },
  },
  {
    kind: 'terminal',
    tier: 'specific',
    multiple: true,
    title: () => '终端',
    guide: {
      order: 20,
      title: '新建终端',
      description: '在会话工作区运行命令',
      artwork: 'terminal',
      shortcut: ['Ctrl', '`'],
    },
  },
  {
    kind: 'changes',
    patterns: [`${CHANGES_PREFIX}**`],
    tier: 'specific',
    // 认领前先解析：解析不出来说明这不是一个改动地址，兜底类型才有机会接手。
    canOpen: (address) => parseChangesAddress(address) !== null,
    title: (address) => {
      const parsed = address === null ? null : parseChangesAddress(address);
      return parsed === null ? '本轮改动' : `第 ${parsed.turnNumber} 轮改动`;
    },
  },
  {
    kind: 'preview',
    patterns: ['axon-resource://file/**', '*'],
    tier: 'fallback',
    title: (address) => (address === null ? '预览' : baseName(address)),
  },
];

/**
 * 引导页的入口表，按 `order` 排序。
 *
 * 入口由各类型自己声明，引导页只负责摆：新增一个「可以打开的东西」时，
 * 在它的类型上加一段 `guide` 就够，不必回头改引导页。
 */
export function guideEntries(): Array<GuideEntry & { kind: string }> {
  return REGISTERED.flatMap((type) =>
    type.guide === undefined ? [] : [{ ...type.guide, kind: type.kind }],
  ).sort((left, right) => left.order - right.order);
}

/**
 * 一个页类型的声明；不是页类型（认领地址的类型）时返回 null。
 *
 * 页类型没有资源身份，因此它的 `patterns` 为空：这一条同时是「谁能当页打开」的判据。
 */
export function pageType(kind: string): TabType | null {
  return (
    REGISTERED.find((type) => type.kind === kind && (type.patterns ?? []).length === 0) ?? null
  );
}

/** 一个改动对比地址解析出的坐标。 */
export interface ChangesAddress {
  /** 轮次在会话里的序号（从 1 开始），只用于展示。 */
  turnNumber: number;
  /** 轮次标识，用于在投影里找回这一轮的改动。 */
  turnId: string;
}

/** 把轮次标识与序号变成改动对比地址。 */
export function changesAddress(turnId: string, turnNumber: number): string {
  return `${CHANGES_PREFIX}${turnNumber}/${encodeURIComponent(turnId)}`;
}

/** 资源地址还原成改动坐标；不是改动地址或格式不合法时返回 null。 */
export function parseChangesAddress(address: string): ChangesAddress | null {
  if (!address.startsWith(CHANGES_PREFIX)) return null;
  const remainder = address.slice(CHANGES_PREFIX.length);
  const separator = remainder.indexOf('/');
  if (separator <= 0) return null;
  const turnNumber = Number.parseInt(remainder.slice(0, separator), 10);
  if (!Number.isSafeInteger(turnNumber) || turnNumber <= 0) return null;
  try {
    const turnId = decodeURIComponent(remainder.slice(separator + 1));
    return turnId.length === 0 ? null : { turnNumber, turnId };
  } catch {
    return null;
  }
}

/** 把文件相对路径变成资源地址。 */
export function fileAddress(path: string): string {
  return `${FILE_PREFIX}${encodeURIComponent(path)}`;
}

/** 资源地址还原成文件相对路径；不是文件地址时返回 null。 */
export function addressPath(address: string): string | null {
  if (!address.startsWith(FILE_PREFIX)) return null;
  try {
    return decodeURIComponent(address.slice(FILE_PREFIX.length));
  } catch {
    return null;
  }
}

/** 地址是否是本套资源协议的地址。 */
export function isResourceAddress(address: string): boolean {
  return address.startsWith('axon-resource://');
}

/** 把 `*` 通配模式编译成匹配器；含 `:` 的模式匹配整个地址，否则匹配路径的任意深度。 */
function matcher(pattern: string): (address: string) => boolean {
  const whole = pattern.includes(':');
  const source = wildcardSource(pattern);
  const expression = whole ? new RegExp(`^${source}$`, 'i') : new RegExp(`(^|/)${source}$`, 'i');
  return (address) => {
    if (whole) return expression.test(address);
    const path = addressPath(address);
    return path === null ? false : expression.test(path.replace(/\\/g, '/'));
  };
}

/**
 * 把通配模式编译成正则片段。
 *
 * 单个 `*` 只吃一段（不跨 `/`），连续两个 `**` 表示任意深度，因此整地址模式
 * 可以承载多段地址（例如改动对比地址里的轮次序号与轮次标识）。
 */
function wildcardSource(pattern: string): string {
  let source = '';
  for (let at = 0; at < pattern.length; at += 1) {
    const character = pattern[at];
    if (character === '*') {
      if (pattern[at + 1] === '*') {
        source += '.*';
        at += 1;
      } else {
        source += '[^/]*';
      }
      continue;
    }
    source += character.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }
  return source;
}

/** 命中模式的长度：越具体的模式越先认领。 */
function patternWeight(type: TabType, address: string): number {
  let best = -1;
  for (const pattern of type.patterns ?? []) {
    if (matcher(pattern)(address)) best = Math.max(best, pattern.replace(/\*/g, '').length);
  }
  return best;
}

/**
 * 裁决由哪个类型打开该地址。
 *
 * 排序依次是：认领档位、模式具体程度、注册顺序；`canOpen` 是否决而不是排序依据。
 */
export function claim(address: string): TabType | null {
  const candidates = REGISTERED.filter(
    (type) => (type.patterns ?? []).some((pattern) => matcher(pattern)(address)),
  ).sort((left, right) => {
    const tier = (type: TabType) => (type.tier === 'fallback' ? 0 : 1);
    if (tier(left) !== tier(right)) return tier(right) - tier(left);
    const weight = patternWeight(right, address) - patternWeight(left, address);
    if (weight !== 0) return weight;
    return REGISTERED.indexOf(left) - REGISTERED.indexOf(right);
  });
  for (const candidate of candidates) {
    if (candidate.canOpen === undefined || candidate.canOpen(address)) return candidate;
  }
  return null;
}
