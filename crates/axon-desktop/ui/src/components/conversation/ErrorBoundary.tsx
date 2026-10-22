import React from 'react';
import { AlertCircleIcon, RetryIcon } from '../../icons';
import { Text } from '../ui/surfaces';

/*
 * 主区域错误边界。
 *
 * 对话流的事实源是外部推送的帧，一帧结构异常不应把整个外壳一起带走：
 * 边界只兜住主区域，并把真实错误原文露出来，同时保留重试入口。
 */

interface ErrorBoundaryProps {
  /** 边界内重置时调用的回调，用于让承载方重新取数。 */
  onReset?: () => void;
  children: React.ReactNode;
}

interface ErrorBoundaryState {
  message: string | null;
}

export class ErrorBoundary extends React.Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { message: null };
  }

  static getDerivedStateFromError(error: unknown): ErrorBoundaryState {
    return { message: error instanceof Error ? error.message : '渲染失败' };
  }

  componentDidCatch(error: unknown): void {
    // 控制台留痕便于真机排查；界面只呈现可读结论。
    console.error('[axon] 主区域渲染失败', error);
  }

  render(): React.ReactNode {
    const { message } = this.state;
    if (message === null) return this.props.children;
    return (
      <div className="flex-1 min-h-0 flex items-center justify-center px-6">
        <div className="max-w-[420px] text-center">
          <AlertCircleIcon size={16} className="text-danger mx-auto" />
          <Text strong className="mt-3 block">
            主区域渲染失败
          </Text>
          <Text tone="secondary" size="meta" className="mt-2 block break-words">
            {message}
          </Text>
          <button
            type="button"
            onClick={() => {
              this.setState({ message: null });
              this.props.onReset?.();
            }}
            className="mt-4 h-8 px-3 inline-flex items-center gap-1.5 rounded-sm hairline border-border-strong bg-surface text-row text-text-primary transition-colors duration-100 hover:bg-hover"
          >
            <RetryIcon size={14} />
            重新加载会话
          </button>
        </div>
      </div>
    );
  }
}
