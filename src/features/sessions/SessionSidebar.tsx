import { useMemo, useState } from 'react';
import type { SessionProfile } from '../../bindings/types';

interface SessionSidebarProps {
  sessions: SessionProfile[];
  onOpenLocal: () => void;
  onOpenSession: (session: SessionProfile) => void;
  onQuickConnect: () => void;
}

export function SessionSidebar({ sessions, onOpenLocal, onOpenSession, onQuickConnect }: SessionSidebarProps) {
  const [query, setQuery] = useState('');
  const normalizedQuery = query.trim().toLowerCase();
  const filteredSessions = useMemo(() => {
    if (!normalizedQuery) return sessions;
    return sessions.filter((session) => {
      const searchable = [
        session.name,
        session.host ?? '',
        session.username ?? '',
        session.protocol,
        ...session.tags,
      ].join(' ').toLowerCase();
      return searchable.includes(normalizedQuery);
    });
  }, [normalizedQuery, sessions]);

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
        <input
          type="search"
          placeholder="过滤会话…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
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
        {sessions.length === 0 ? (
          <p className="empty-state">暂无保存的 SSH 会话。使用快速连接后可保存为正式会话。</p>
        ) : filteredSessions.length === 0 ? (
          <p className="empty-state">没有匹配“{query}”的会话。</p>
        ) : (
          filteredSessions.map((session) => (
            <button type="button" className="session-node" key={session.id} onClick={() => onOpenSession(session)}>
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
