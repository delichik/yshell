import type { SessionProfile } from '../../bindings/types';

interface SessionSidebarProps {
  sessions: SessionProfile[];
  onQuickConnect: () => void;
}

export function SessionSidebar({ sessions, onQuickConnect }: SessionSidebarProps) {
  return (
    <aside className="session-sidebar" aria-label="会话树">
      <div className="sidebar-header">
        <div>
          <span className="eyebrow">Sessions</span>
          <h2>会话树</h2>
        </div>
        <button type="button" onClick={onQuickConnect} aria-label="添加连接">＋</button>
      </div>
      <label className="search-box">
        <span>搜索名称、主机、标签</span>
        <input type="search" placeholder="过滤会话…" />
      </label>
      <section className="session-group">
        <h3>本地</h3>
        <button type="button" className="session-node">
          <span className="node-icon">⌘</span>
          <span>
            <strong>默认 Shell</strong>
            <small>使用系统登录 Shell</small>
          </span>
        </button>
      </section>
      <section className="session-group">
        <h3>远程连接</h3>
        {sessions.length === 0 ? (
          <p className="empty-state">暂无保存的 SSH 会话。使用快速连接后可保存为正式会话。</p>
        ) : (
          sessions.map((session) => (
            <button type="button" className="session-node" key={session.id}>
              <span className="node-icon">{session.protocol === 'ssh' ? 'SSH' : '⌘'}</span>
              <span>
                <strong>{session.name}</strong>
                <small>{session.host ? `${session.username ?? 'user'}@${session.host}:${session.port ?? 22}` : 'local'}</small>
              </span>
            </button>
          ))
        )}
      </section>
    </aside>
  );
}
