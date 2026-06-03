import type { WorkspacePane } from '../bindings/types';

interface StatusBarProps {
  pane?: WorkspacePane;
  loggingEnabled: boolean;
  message?: string | null;
}

export function StatusBar({ pane, loggingEnabled, message }: StatusBarProps) {
  return (
    <footer className="status-bar">
      <span>状态：{pane?.status ?? 'idle'}</span>
      <span>窗格：{pane?.title ?? '未选择'}</span>
      <span>尺寸：自适应</span>
      <span>编码：UTF-8</span>
      <span>日志：{loggingEnabled ? '开启' : '关闭'}</span>
      <span>广播输入：关闭</span>
      {message && <span className="status-message">{message}</span>}
    </footer>
  );
}
