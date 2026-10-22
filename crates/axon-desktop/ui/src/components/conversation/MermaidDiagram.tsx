import React from 'react';

/*
 * 图形渲染：mermaid。
 *
 * 流程图、时序图、状态图这类内容用文字描述既难写也难读，mermaid 让模型
 * 直接输出图形定义，这里渲染成 SVG。渲染引擎体量大，只在正文里真的出现
 * 图形时才按需下载，普通会话不付这份代价。
 *
 * 安全基线：securityLevel 保持 strict（引擎默认），标签按纯文本处理，
 * 渲染结果不执行任何脚本；同时关闭引擎自带的错误图形注入，失败由调用方
 * 退回源码展示，避免页面上多出一块不可控的临时节点。
 */

/** mermaid 默认导出（渲染 API）。 */
type MermaidApi = (typeof import('mermaid'))['default'];

/** 引擎单例：初始化一次，后续复用。 */
let engine: Promise<MermaidApi> | null = null;

/** 渲染用的唯一 id 计数器。 */
let sequence = 0;

/**
 * 读一个设计令牌的当前取值。
 *
 * 图形的调色与字体与界面同源：令牌是唯一事实源，这里不落第二份副本，
 * 因此换主题时图形跟着一起变。
 */
function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function bootstrap(): Promise<MermaidApi> {
  engine ??= import('mermaid').then((module) => {
    const mermaid = module.default;
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      // 解析失败时不要往页面里注入错误图形，失败交给调用方处理。
      suppressErrorRendering: true,
      theme: 'base',
      fontFamily: token('--font-sans'),
      themeVariables: {
        background: token('--color-surface'),
        primaryColor: token('--color-surface-sunken'),
        primaryBorderColor: token('--color-border-selected'),
        primaryTextColor: token('--color-text-primary'),
        secondaryColor: token('--color-surface-tint'),
        tertiaryColor: token('--color-code-block'),
        lineColor: token('--color-text-tertiary'),
        textColor: token('--color-text-primary'),
        fontSize: token('--text-row'),
      },
      flowchart: { curve: 'basis', htmlLabels: false, useMaxWidth: true },
      sequence: { useMaxWidth: true },
    });
    return mermaid;
  });
  return engine;
}

/** 清掉引擎可能留在文档里的临时测量节点。 */
function cleanup(id: string): void {
  for (const candidate of [id, `d${id}`]) {
    const node = document.getElementById(candidate);
    if (node !== null && node.parentElement === document.body) node.remove();
  }
}

export interface MermaidDiagramProps {
  /** 图形定义源码。 */
  code: string;
  /** 渲染失败回调：由调用方决定怎么展示原始定义。 */
  onError: (message: string) => void;
}

export const MermaidDiagram: React.FC<MermaidDiagramProps> = ({ code, onError }) => {
  const [svg, setSvg] = React.useState<string | null>(null);
  const failure = React.useRef(onError);
  failure.current = onError;

  React.useEffect(() => {
    let cancelled = false;
    setSvg(null);
    const id = `axon-mermaid-${(sequence += 1)}`;
    void bootstrap()
      .then((mermaid) => mermaid.render(id, code))
      .then((result) => {
        if (!cancelled) setSvg(result.svg);
      })
      .catch((error: unknown) => {
        cleanup(id);
        if (!cancelled) {
          failure.current(error instanceof Error ? error.message : '图形渲染失败');
        }
      })
      .finally(() => cleanup(id));
    return () => {
      cancelled = true;
    };
  }, [code]);

  // 外框与留白由调用方的代码块容器提供，这里只负责图形本体。
  if (svg === null) {
    return (
      <div className="px-4 py-3">
        <span className="text-code text-text-caption">图形渲染中</span>
      </div>
    );
  }

  // mermaid 输出的是自绘 SVG，且已按 strict 级别过滤标签，这里直接内联。
  return (
    <div
      className="px-4 py-3 overflow-x-auto"
      data-diagram="mermaid"
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
};
