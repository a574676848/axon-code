/*
 * 工作区文件与终端的传输模型。
 *
 * 单独成篇的原因只有一个：`types.ts` 是 IPC 合约的总出口，模型按域拆开才守得住
 * 单文件行数硬限。调用方仍旧只从 `types` 取模型，这里由 `types.ts` 整体回导出。
 */

/** 工作区内的一个文件或目录。 */
export interface FileEntryItem {
  name: string;
  /** 相对工作区根目录的路径，统一正斜杠。 */
  path: string;
  /** dir 或 file。 */
  kind: string;
}

/** 工作区内一个文本文件的内容。 */
export interface FileTextItem {
  /** 相对工作区根目录的路径，统一正斜杠。 */
  path: string;
  /** 工作区内的绝对路径；查看页的路径栏显示这一份。 */
  absolute_path: string;
  content: string;
  truncated: boolean;
  binary: boolean;
}

/** 工作区内一个文件的原始字节。 */
export interface FileBytesItem {
  /** 相对工作区根目录的路径，统一正斜杠。 */
  path: string;
  /** 工作区内的绝对路径。 */
  absolute_path: string;
  /** 由扩展名判定的 MIME 类型。 */
  mime: string;
  /** 原始字节的 base64。 */
  data_base64: string;
  /** 是否因超出上限被截断。 */
  truncated: boolean;
}

/** 系统为某个文件登记的一条打开方式。 */
export interface FileApplicationItem {
  /** 平台原生标识：Windows 是关联处理程序的 ProgID／AppID，Linux 是桌面条目 id。 */
  id: string;
  /** 展示名；系统没有给出名字时是标识的文件名部分。 */
  name: string;
  /** 图标：`data:image/png;base64,...`；取不到时为 null。 */
  icon: string | null;
  /** 是否为系统认定的默认打开方式。 */
  default: boolean;
}

/** 本机可用的一个 shell。 */
export interface ShellChoiceItem {
  /** 启动用的程序：绝对路径，或 PATH 上的命令名。 */
  path: string;
  /** 展示名。 */
  name: string;
}

/** 已建立的终端会话。 */
export interface TerminalInfoItem {
  id: string;
  shell: string;
  cwd: string;
  cols: number;
  rows: number;
}

/** 一帧终端输出；`data` 是原始字节的 base64。 */
export interface TerminalOutputFrame {
  id: string;
  data: string;
}

/** 一次终端退出。 */
export interface TerminalExitFrame {
  id: string;
  /** 退出码；拿不到时为 null。 */
  code: number | null;
  /** 是否由用户主动关闭。 */
  closed: boolean;
}
