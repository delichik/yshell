import { FormEvent, useMemo, useState } from 'react';
import type { AuthMethod, HostKeyPolicy, QuickConnectDraft, SessionProfile, SessionProtocol } from '../../bindings/types';

interface QuickConnectPanelProps {
  initialSession?: SessionProfile | null;
  mode?: 'connect' | 'edit';
  onCancel: () => void;
  onConnect: (draft: QuickConnectDraft) => Promise<void>;
}

type PropertyPage = 'general' | 'connection' | 'auth' | 'terminal' | 'logging';

const defaultDraft: QuickConnectDraft = {
  protocol: 'ssh',
  name: '',
  host: '',
  port: 22,
  username: '',
  description: '',
  tags: [],
  folderId: null,
  color: '#3b82f6',
  favorite: false,
  authMethod: 'password',
  privateKeyPath: '',
  password: '',
  hostKeyPolicy: 'prompt',
  saveAsSession: true,
};

const propertyPages: Array<{ id: PropertyPage; label: string; description: string }> = [
  { id: 'general', label: '常规', description: '名称、协议和连接摘要' },
  { id: 'connection', label: '连接', description: '主机、端口、用户名和主机密钥' },
  { id: 'auth', label: '用户身份验证', description: '密码、私钥或 Agent' },
  { id: 'terminal', label: '终端', description: '终端类型、编码和启动行为' },
  { id: 'logging', label: '日志', description: '会话日志策略' },
];

export function QuickConnectPanel({ initialSession, mode = 'connect', onCancel, onConnect }: QuickConnectPanelProps) {
  const initialDraft = useMemo(() => sessionToDraft(initialSession), [initialSession]);
  const [activePage, setActivePage] = useState<PropertyPage>('general');
  const [protocol, setProtocol] = useState<SessionProtocol>(initialDraft.protocol);
  const [name, setName] = useState(initialDraft.name);
  const [host, setHost] = useState(initialDraft.host);
  const [port, setPort] = useState(initialDraft.port);
  const [username, setUsername] = useState(initialDraft.username);
  const [description, setDescription] = useState(initialDraft.description ?? '');
  const [tagsDraft, setTagsDraft] = useState((initialDraft.tags ?? []).join(', '));
  const [folderId, setFolderId] = useState(initialDraft.folderId ?? '');
  const [color, setColor] = useState(initialDraft.color ?? '#3b82f6');
  const [favorite, setFavorite] = useState(Boolean(initialDraft.favorite));
  const [authMethod, setAuthMethod] = useState<AuthMethod>(initialDraft.authMethod);
  const [password, setPassword] = useState(initialDraft.password ?? '');
  const [privateKeyPath, setPrivateKeyPath] = useState(initialDraft.privateKeyPath ?? '');
  const [hostKeyPolicy, setHostKeyPolicy] = useState<HostKeyPolicy>(initialDraft.hostKeyPolicy);
  const [saveAsSession, setSaveAsSession] = useState(mode === 'edit' ? true : initialDraft.saveAsSession);
  const [connecting, setConnecting] = useState(false);

  const isSsh = protocol === 'ssh';
  const derivedName = isSsh ? name || `${username || 'user'}@${host || 'host'}` : name || '本地终端';

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setConnecting(true);
    try {
      await onConnect({
        protocol,
        name: derivedName,
        host,
        port,
        username,
        description: description.trim() || undefined,
        tags: tagsDraft.split(',').map((tag) => tag.trim()).filter(Boolean),
        folderId: folderId.trim() || null,
        color,
        favorite,
        authMethod,
        password: authMethod === 'password' ? password : undefined,
        privateKeyPath: authMethod === 'private_key' ? privateKeyPath : undefined,
        hostKeyPolicy,
        saveAsSession,
      });
    } finally {
      setConnecting(false);
    }
  };

  return (
    <div className="dialog-backdrop" role="presentation">
      <form className="dialog session-properties" onSubmit={submit}>
        <header className="properties-header">
          <div>
            <span className="eyebrow">{mode === 'edit' ? 'Session Properties' : 'New Session Properties'}</span>
            <h2>{mode === 'edit' ? '会话属性' : '新建 / 快速连接'}</h2>
            <p>以 Xshell 的“会话属性”为基线组织配置；当前版本只开放 SSH 与本地 Shell，其他协议不在本阶段范围内。</p>
          </div>
          <div className="properties-summary" aria-label="会话摘要">
            <strong>{derivedName}</strong>
            <span>{isSsh ? `${host || '未设置主机'}:${port}` : '本地 Shell'}</span>
          </div>
        </header>

        <div className="properties-layout">
          <nav className="properties-nav" aria-label="会话属性分类">
            {propertyPages.map((page) => (
              <button key={page.id} type="button" data-active={activePage === page.id} onClick={() => setActivePage(page.id)}>
                <strong>{page.label}</strong>
                <span>{page.description}</span>
              </button>
            ))}
          </nav>

          <section className="properties-page" aria-live="polite">
            {activePage === 'general' && (
              <>
                <h3>常规</h3>
                <div className="form-grid">
                  <label>
                    名称
                    <input value={name} onChange={(event) => setName(event.target.value)} placeholder="生产跳板机" />
                  </label>
                  <label>
                    描述
                    <input value={description} onChange={(event) => setDescription(event.target.value)} placeholder="用途、环境、负责人" />
                  </label>
                  <label>
                    标签
                    <input value={tagsDraft} onChange={(event) => setTagsDraft(event.target.value)} placeholder="prod, db, cn-north" />
                  </label>
                  <label>
                    文件夹
                    <input value={folderId} onChange={(event) => setFolderId(event.target.value)} placeholder="生产环境/数据库" />
                  </label>
                  <label>
                    颜色
                    <input type="color" value={color} onChange={(event) => setColor(event.target.value)} />
                  </label>
                  <label className="checkbox-row inline-checkbox">
                    <input type="checkbox" checked={favorite} onChange={(event) => setFavorite(event.target.checked)} />
                    加入收藏
                  </label>
                  <label>
                    协议
                    <select value={protocol} onChange={(event) => setProtocol(event.target.value as SessionProtocol)}>
                      <option value="ssh">SSH</option>
                      <option value="local">本地 Shell</option>
                    </select>
                  </label>
                </div>
                <div className="property-callout">
                  <strong>产品边界</strong>
                  <p>YShell 的目标是专业会话工作台，不是临时 shell 面板。会话属性必须能保存非敏感配置，敏感凭据走安全边界。</p>
                </div>
              </>
            )}

            {activePage === 'connection' && (
              <>
                <h3>连接</h3>
                {isSsh ? (
                  <div className="form-grid">
                    <label>
                      主机
                      <input required value={host} onChange={(event) => setHost(event.target.value)} placeholder="example.com" />
                    </label>
                    <label>
                      端口
                      <input type="number" min={1} max={65535} value={port} onChange={(event) => setPort(Number(event.target.value))} />
                    </label>
                    <label>
                      用户名
                      <input value={username} onChange={(event) => setUsername(event.target.value)} placeholder="当前系统用户" />
                    </label>
                    <label>
                      主机密钥策略
                      <select value={hostKeyPolicy} onChange={(event) => setHostKeyPolicy(event.target.value as HostKeyPolicy)}>
                        <option value="prompt">未知或变化时提示</option>
                        <option value="accept_new">自动信任新主机</option>
                        <option value="strict">仅连接已信任主机</option>
                      </select>
                    </label>
                  </div>
                ) : (
                  <div className="property-callout"><strong>本地 Shell</strong><p>本地连接使用设置中的默认 shell、工作目录、字体和主题。</p></div>
                )}
              </>
            )}

            {activePage === 'auth' && (
              <>
                <h3>用户身份验证</h3>
                {isSsh ? (
                  <>
                    <div className="form-grid">
                      <label>
                        方法
                        <select value={authMethod} onChange={(event) => setAuthMethod(event.target.value as AuthMethod)}>
                          <option value="password">Password / Keyboard-interactive</option>
                          <option value="private_key">Public Key</option>
                          <option value="agent">SSH Agent</option>
                        </select>
                      </label>
                      {authMethod === 'password' && (
                        <label>
                          密码
                          <input
                            autoComplete="current-password"
                            type="password"
                            value={password}
                            onChange={(event) => setPassword(event.target.value)}
                            placeholder="可留空，连接时在终端输入"
                          />
                        </label>
                      )}
                      {authMethod === 'private_key' && (
                        <label>
                          私钥路径
                          <input value={privateKeyPath} onChange={(event) => setPrivateKeyPath(event.target.value)} placeholder="~/.ssh/id_ed25519" />
                        </label>
                      )}
                    </div>
                    <div className="property-callout security">
                      <strong>密码处理策略</strong>
                      <p>本次连接填写的密码只用于自动回应 OpenSSH 的密码提示，不写入会话配置、不进入导出文件。保存会话时仅保存“使用密码认证”的方式。</p>
                    </div>
                  </>
                ) : (
                  <div className="property-callout"><strong>无需 SSH 认证</strong><p>本地 Shell 不需要远程认证配置。</p></div>
                )}
              </>
            )}

            {activePage === 'terminal' && (
              <>
                <h3>终端</h3>
                <div className="property-callout">
                  <strong>继承全局终端设置</strong>
                  <p>当前会话继承字体、字号、行高、回滚行数和主题。后续应在此提供会话级覆盖，而不是把设置散落在工具栏。</p>
                </div>
              </>
            )}

            {activePage === 'logging' && (
              <>
                <h3>日志</h3>
                <div className="property-callout">
                  <strong>继承日志策略</strong>
                  <p>当前版本使用全局日志策略；完整 Xshell 复刻要求会话级自动日志、文件名模板、追加/覆盖和脱敏开关。</p>
                </div>
              </>
            )}
          </section>
        </div>

        {mode === 'connect' && (
          <label className="checkbox-row save-session-row">
            <input type="checkbox" checked={saveAsSession} onChange={(event) => setSaveAsSession(event.target.checked)} />
            保存为会话配置（保存主机、端口、用户名、认证方式；不保存明文密码）
          </label>
        )}
        <footer>
          <button type="button" onClick={onCancel}>取消</button>
          <button type="submit" className="primary" disabled={connecting}>{connecting ? '处理中…' : mode === 'edit' ? '保存' : '连接'}</button>
        </footer>
      </form>
    </div>
  );
}

function sessionToDraft(session?: SessionProfile | null): QuickConnectDraft {
  if (!session) return defaultDraft;
  return {
    protocol: session.protocol,
    name: session.name,
    host: session.host ?? '',
    port: session.port ?? 22,
    username: session.username ?? session.auth.username ?? '',
    description: session.description ?? '',
    tags: session.tags ?? [],
    folderId: session.folderId,
    color: session.color ?? '#3b82f6',
    favorite: Boolean(session.favorite),
    authMethod: session.auth.method,
    privateKeyPath: session.auth.privateKeyPath ?? '',
    password: '',
    hostKeyPolicy: 'prompt',
    saveAsSession: true,
  };
}
