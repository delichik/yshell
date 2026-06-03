import { type CSSProperties, useMemo, useState } from 'react';
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

interface SessionGroup {
  id: string;
  title: string;
  sessions: SessionProfile[];
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
      [session.name, session.host, session.username, session.protocol, session.description, session.color, ...session.tags]
        .filter(Boolean)
        .some((value) => String(value).toLowerCase().includes(normalized)),
    );
  }, [query, sessions]);

  const favoriteSessions = filteredSessions.filter((session) => session.favorite);
  const groupedSessions = useMemo<SessionGroup[]>(() => {
    const groups = new Map<string, SessionProfile[]>();
    for (const session of filteredSessions) {
      const groupName = session.folderId?.trim() || '未分类会话';
      groups.set(groupName, [...(groups.get(groupName) ?? []), session]);
    }
    return [...groups.entries()].map(([title, groupSessions]) => ({
      id: title,
      title,
      sessions: groupSessions.sort(compareSessions),
    }));
  }, [filteredSessions]);

  return (
    <aside className="session-sidebar" aria-label="会话管理器">
      <div className="sidebar-header session-manager-header">
        <div>
          <span className="eyebrow">Session Manager</span>
          <h2>会话管理器</h2>
          <p>保存、检索、分组和审视 SSH 会话，而不是只维护一个临时连接列表。</p>
        </div>
        <button type="button" onClick={onQuickConnect} aria-label="新建会话">＋</button>
      </div>
      <div className="session-manager-actions" aria-label="会话管理快捷操作">
        <button type="button" onClick={onQuickConnect}>新建 SSH</button>
        <button type="button" onClick={onOpenLocal}>本地 Shell</button>
      </div>
      <label className="search-box">
        <span>搜索名称、主机、用户、标签、描述</span>
        <input type="search" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="过滤会话…" />
      </label>

      {favoriteSessions.length > 0 && (
        <section className="session-group session-group-favorites">
          <h3>收藏</h3>
          {favoriteSessions.map((session) => renderSessionCard(session, { onOpenSession, onEditSession, onDuplicateSession, onDeleteSession }))}
        </section>
      )}

      <section className="session-group">
        <h3>本地</h3>
        <button type="button" className="session-node local-node" onClick={onOpenLocal}>
          <span className="node-icon">⌘</span>
          <span>
            <strong>默认 Shell</strong>
            <small>使用系统登录 Shell 与全局终端设置</small>
          </span>
        </button>
      </section>

      <section className="session-group">
        <h3>远程连接 <span className="session-count">{filteredSessions.length}</span></h3>
        {filteredSessions.length === 0 ? (
          <p className="empty-state">暂无匹配的 SSH 会话。请用“新建 SSH”创建包含主机、认证、标签和安全策略的正式会话。</p>
        ) : (
          groupedSessions.map((group) => (
            <section className="session-folder" key={group.id} aria-label={group.title}>
              <h4>▾ {group.title} <span className="session-count">{group.sessions.length}</span></h4>
              {group.sessions.map((session) => renderSessionCard(session, { onOpenSession, onEditSession, onDuplicateSession, onDeleteSession }))}
            </section>
          ))
        )}
      </section>
    </aside>
  );
}

function renderSessionCard(
  session: SessionProfile,
  handlers: Pick<SessionSidebarProps, 'onOpenSession' | 'onEditSession' | 'onDuplicateSession' | 'onDeleteSession'>,
) {
  const endpoint = session.host ? `${session.username ?? 'user'}@${session.host}:${session.port ?? 22}` : 'local';
  return (
    <article className="session-card" key={session.id} style={{ '--session-color': session.color ?? '#3b82f6' } as CSSProperties}>
      <button type="button" className="session-node" onClick={() => handlers.onOpenSession(session)}>
        <span className="node-icon">{session.favorite ? '★' : session.protocol === 'ssh' ? 'SSH' : '⌘'}</span>
        <span className="session-node-main">
          <strong>{session.name}</strong>
          <small>{endpoint}</small>
          {session.description && <small className="session-description">{session.description}</small>}
          {session.tags.length > 0 && (
            <span className="session-tags" aria-label="标签">
              {session.tags.map((tag) => <em key={tag}>{tag}</em>)}
            </span>
          )}
        </span>
      </button>
      <div className="session-actions" aria-label={`${session.name} 操作`}>
        <button type="button" onClick={() => handlers.onOpenSession(session)}>打开</button>
        <button type="button" onClick={() => handlers.onEditSession(session)}>属性</button>
        <button type="button" onClick={() => handlers.onDuplicateSession(session)}>复制</button>
        <button type="button" onClick={() => handlers.onDeleteSession(session)}>删除</button>
      </div>
    </article>
  );
}

function compareSessions(left: SessionProfile, right: SessionProfile) {
  if (left.favorite !== right.favorite) return left.favorite ? -1 : 1;
  return left.name.localeCompare(right.name, 'zh-Hans-CN');
}
