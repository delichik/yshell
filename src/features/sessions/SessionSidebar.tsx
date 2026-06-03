import { useMemo, useState } from 'react';
import type { SessionProfile } from '../../bindings/types';

interface SessionSidebarProps {
  sessions: SessionProfile[];
  onQuickConnect: () => void;
  onOpenLocal: () => void;
  onOpenSession: (session: SessionProfile) => void;
  onEditSession: (session: SessionProfile) => void;
  onDuplicateSession: (session: SessionProfile) => void;
  onDeleteSession: (session: SessionProfile) => void;
}

export function SessionSidebar({
  sessions,
  onQuickConnect,
  onOpenLocal,
  onOpenSession,
  onEditSession,
  onDuplicateSession,
  onDeleteSession,
}: SessionSidebarProps) {
  const [query, setQuery] = useState('');
  const filteredSessions = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (!normalized) return sessions;
    return sessions.filter((session) =>
      [session.name, session.host, session.username, session.protocol, ...session.tags]
        .filter(Boolean)
        .some((value) => String(value).toLowerCase().includes(normalized)),
    );
  }, [query, sessions]);

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
        <input type="search" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="过滤会话…" />
      </label>
      <section className="session-group">
        <h3>本地</h3>
        <button type="button" className="session-node" onClick={onOpenLocal}>
          <span className="node-icon">⌘</span>
          <span>
            <strong>默认 Shell</strong>
            <small>使用系统登录 Shell</small>
          </span>
        </button>
      </section>
      <section className="session-group">
        <h3>远程连接</h3>
        {filteredSessions.length === 0 ? (
          <p className="empty-state">暂无匹配的 SSH 会话。使用快速连接后可保存为正式会话。</p>
        ) : (
          filteredSessions.map((session) => (
            <article className="session-card" key={session.id}>
              <button type="button" className="session-node" onClick={() => onOpenSession(session)}>
                <span className="node-icon">{session.protocol === 'ssh' ? 'SSH' : '⌘'}</span>
                <span>
                  <strong>{session.name}</strong>
                  <small>{session.host ? `${session.username ?? 'user'}@${session.host}:${session.port ?? 22}` : 'local'}</small>
                </span>
              </button>
              <div className="session-actions" aria-label={`${session.name} 操作`}>
                <button type="button" onClick={() => onEditSession(session)}>编辑</button>
                <button type="button" onClick={() => onDuplicateSession(session)}>复制</button>
                <button type="button" onClick={() => onDeleteSession(session)}>删除</button>
              </div>
            </article>
          ))
        )}
      </section>
    </aside>
  );
}
