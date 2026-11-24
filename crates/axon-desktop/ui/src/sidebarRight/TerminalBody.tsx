import React from 'react';
import { Terminal } from '@xterm/xterm';
import type { ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import * as ipc from '../ipc';
import { PlusIcon, TerminalIcon } from '../icons';
import { EmptyState } from '../components/ui/surfaces';
import { currentTheme, onThemeChange } from '../theme';

/*
 * 终端正文：一个会话一个真实 PTY，屏幕由 xterm 绘制。
 *
 * 后端只搬字节，因此这里不做任何仿真：输出按原始字节写入（base64 解码后交给 xterm，
 * 半截的多字节字符不会被拆坏），键盘输入按 UTF-8 原样回传。
 *
 * 输出通道先于终端建立：会话标识要等 `terminal_open` 返回才知道，而 shell 的欢迎信息
 * 可能在那之前就到了，因此先到的帧按下暂存，拿到标识后再补写，不丢开头几帧。
 *
 * 尺寸双向同步：容器变化时先按实际像素算出行列、把屏幕设成同一尺寸，再把尺寸转给 PTY，
 * 全屏程序（vim、top）据此重排。
 */

/** 终端字体：与 `--font-mono` 同一族，取不到时回落到系统等宽。 */
const TERMINAL_FONT =
  "'SF Mono', 'JetBrains Mono', 'Fira Code', Consolas, 'Liberation Mono', Menlo, Courier, 'PingFang SC', 'Microsoft YaHei'";

/** 回滚缓冲行数。 */
const SCROLLBACK_LINES = 4000;

/** 终端字号。 */
const TERMINAL_FONT_SIZE = 13;

/** 从设计令牌读一个颜色；令牌缺失时用同一语义的兜底值。 */
function readToken(name: string, fallback: string): string {
  const value = window.getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value.length > 0 ? value : fallback;
}

/** 解析 #rrggbb 与 rgb()/rgba() 两种写法；解析失败返回 null。 */
function parseColor(value: string): { r: number; g: number; b: number; a: number } | null {
  const hex = /^#([0-9a-f]{6})$/i.exec(value);
  if (hex !== null) {
    const int = Number.parseInt(hex[1], 16);
    return { r: (int >> 16) & 255, g: (int >> 8) & 255, b: int & 255, a: 1 };
  }
  const functional = /^rgba?\(([^)]+)\)$/i.exec(value);
  if (functional === null) return null;
  const parts = functional[1].split(/[\s,/]+/).filter(Boolean).map(Number);
  if (parts.length < 3 || parts.slice(0, 3).some((part) => !Number.isFinite(part))) return null;
  return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
}

/**
 * 把半透明色压到不透明底色上。
 *
 * xterm 只在开启透明合成时才认 alpha，而透明合成会让滚动的每一帧都多一次合成；
 * 选区底本来就是「底白加一点蓝」，压平之后颜色一模一样，代价却少一半。
 */
function flatten(color: string, background: string): string {
  const front = parseColor(color);
  const back = parseColor(background);
  if (front === null || back === null || front.a >= 1) return color;
  const mix = (a: number, b: number): number => Math.round(a * front.a + b * (1 - front.a));
  const hex = (value: number): string => value.toString(16).padStart(2, '0');
  return `#${hex(mix(front.r, back.r))}${hex(mix(front.g, back.g))}${hex(mix(front.b, back.b))}`;
}

/**
 * 终端主题：随当前工作台主题切换浅色或黑曜石深色色板。
 */
function terminalTheme(): ITheme {
  const dark = currentTheme() === 'obsidian';
  const background = readToken('--color-surface', dark ? '#181c25' : '#ffffff');
  const foreground = readToken('--color-text-primary', dark ? '#eceff4' : '#1c1917');
  const selection = readToken(
    '--color-focus-soft',
    dark ? 'rgba(82, 139, 255, 0.16)' : 'rgba(46, 102, 166, 0.10)',
  );
  return {
    background,
    foreground,
    cursor: foreground,
    cursorAccent: background,
    selectionBackground: flatten(selection, background),
    black: dark ? '#0d1016' : foreground,
    brightBlack: readToken('--color-text-tertiary', dark ? '#8b95a7' : '#615b53'),
    red: readToken('--color-danger', dark ? '#f87171' : '#b91c1c'),
    green: readToken('--color-ok-ink', dark ? '#4ade80' : '#0f6a31'),
    yellow: readToken('--color-gold-ink', dark ? '#fbbf24' : '#8a5200'),
    blue: readToken('--color-text-link', dark ? '#79a6ff' : '#24538a'),
    magenta: readToken('--shiki-token-function', dark ? '#c4b5fd' : '#6345a8'),
    cyan: readToken('--color-chart-cache-write', dark ? '#2dd4bf' : '#2b8a7e'),
    white: readToken('--color-text-secondary', dark ? '#a6b0c0' : '#4a453e'),
    brightWhite: foreground,
  };
}

/** 把一帧 base64 输出还原成原始字节。 */
function decodeBase64(data: string): Uint8Array {
  const binary = window.atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

export interface TerminalBodyProps {
  /** 工作区根目录绝对路径；终端的工作目录。 */
  root: string;
  /** 指定的 shell；缺省由后端按配置顺序取第一个可用的。 */
  shellPath?: string;
  /** 退出后重开一个终端；由外层换一个 tab 实例完成。 */
  onRestart: () => void;
}

export const TerminalBody: React.FC<TerminalBodyProps> = ({ root, shellPath, onRestart }) => {
  const host = React.useRef<HTMLDivElement>(null);
  const [exited, setExited] = React.useState<number | null | undefined>(undefined);
  const [error, setError] = React.useState<string | null>(null);
  const supported = ipc.terminalSupported();

  React.useEffect(() => {
    if (!supported) return undefined;
    const node = host.current;
    if (node === null) return undefined;

    const terminal = new Terminal({
      cursorBlink: true,
      fontSize: TERMINAL_FONT_SIZE,
      fontFamily: TERMINAL_FONT,
      scrollback: SCROLLBACK_LINES,
      minimumContrastRatio: 4.5,
      theme: terminalTheme(),
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(node);
    terminal.focus();

    let id: string | null = null;
    let disposed = false;
    let pending: Array<{ id: string; data: string }> = [];
    const disposers: Array<() => void> = [];

    // xterm 的主题是构造期对象，不跟 CSS 变量走：切主题必须重新读令牌再赋一次。
    disposers.push(
      onThemeChange(() => {
        if (!disposed) terminal.options.theme = terminalTheme();
      }),
    );

    const fitScreen = () => {
      const dimensions = fit.proposeDimensions();
      if (dimensions === undefined || dimensions.cols < 2 || dimensions.rows < 1) return;
      terminal.resize(dimensions.cols, dimensions.rows);
      if (id !== null) {
        void ipc.terminalResize(id, dimensions.cols, dimensions.rows).catch(() => undefined);
      }
    };

    void (async () => {
      // 订阅先于建会话：shell 的欢迎信息可能在建会话的同一步就到达。
      const offOutput = await ipc.listenTerminalOutput((frame) => {
        if (id === null) {
          pending.push(frame);
          return;
        }
        if (frame.id === id) terminal.write(decodeBase64(frame.data));
      });
      const offExit = await ipc.listenTerminalExit((frame) => {
        if (frame.id === id) setExited(frame.code);
      });
      if (disposed) {
        offOutput();
        offExit();
        return;
      }
      disposers.push(offOutput, offExit);

      const dimensions = fit.proposeDimensions();
      try {
        const info = await ipc.terminalOpen(
          root,
          shellPath ?? null,
          dimensions?.cols ?? 80,
          dimensions?.rows ?? 24,
        );
        if (disposed) {
          void ipc.terminalClose(info.id).catch(() => undefined);
          return;
        }
        id = info.id;
        for (const frame of pending) {
          if (frame.id === id) terminal.write(decodeBase64(frame.data));
        }
        pending = [];
        terminal.focus();
        fitScreen();
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : '无法启动终端');
      }
    })();

    const input = terminal.onData((data) => {
      if (id === null) return;
      void ipc.terminalWrite(id, data).catch(() => undefined);
    });
    disposers.push(() => input.dispose());

    const observer = new ResizeObserver(() => fitScreen());
    observer.observe(node);
    disposers.push(() => observer.disconnect());

    return () => {
      disposed = true;
      for (const dispose of disposers) dispose();
      if (id !== null) void ipc.terminalClose(id).catch(() => undefined);
      terminal.dispose();
    };
  }, [root, shellPath, supported]);

  if (!supported) {
    return (
      <EmptyState
        icon={<TerminalIcon size={24} />}
        title="终端需要在桌面应用中使用"
        description="浏览器调试模式没有终端输出通道。"
        className="flex-1"
      />
    );
  }

  return (
    <div data-rail-terminal className="flex-1 min-h-0 flex flex-col gap-2 p-3">
      {error !== null && (
        <div className="shrink-0 rounded-sm bg-danger-soft px-2.5 py-1.5">
          <span className="break-words text-meta text-danger">{error}</span>
        </div>
      )}
      {exited !== undefined && (
        <div className="shrink-0 flex items-center gap-2">
          <span className="flex-1 min-w-0 text-meta text-text-secondary">
            {exited === null ? '终端已退出' : `终端已退出（代码 ${exited}）`}
          </span>
          <button
            type="button"
            onClick={onRestart}
            className="shrink-0 inline-flex items-center gap-1.5 h-7 px-2.5 rounded-sm bg-primary text-canvas text-meta transition-colors duration-100 hover:bg-primary-hover"
          >
            <PlusIcon size={14} />
            新建终端
          </button>
        </div>
      )}
      {/* 终端自己的面：12px 圆角加 0.5px 发丝线，与右侧栏其它卡片同一种收边。 */}
      <div className="flex-1 min-h-0 overflow-hidden rounded-md hairline border-border-hairline bg-surface">
        <div ref={host} className="scroll-visible h-full px-2.5 pt-2 pb-1" />
      </div>
    </div>
  );
};
