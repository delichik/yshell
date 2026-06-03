import { FormEvent, useMemo, useState } from 'react';
import type { AuthMethod, HostKeyPolicy, QuickConnectDraft, SessionProfile, SessionProtocol } from '../../bindings/types';

interface QuickConnectPanelProps {
  initialSession?: SessionProfile | null;
  mode?: 'connect' | 'edit';
  onCancel: () => void;
  onConnect: (draft: QuickConnectDraft) => Promise<void>;
}

const defaultDraft: QuickConnectDraft = {
  protocol: 'ssh',
  name: '',
  host: '',
  port: 22,
  username: '',
  authMethod: 'password',
  privateKeyPath: '',
  hostKeyPolicy: 'prompt',
  saveAsSession: true,
};

export function QuickConnectPanel({ initialSession, mode = 'connect', onCancel, onConnect }: QuickConnectPanelProps) {
  const initialDraft = useMemo(() => sessionToDraft(initialSession), [initialSession]);
  const [protocol, setProtocol] = useState<SessionProtocol>(initialDraft.protocol);
  const [name, setName] = useState(initialDraft.name);
  const [host, setHost] = useState(initialDraft.host);
  const [port, setPort] = useState(initialDraft.port);
  const [username, setUsername] = useState(initialDraft.username);
  const [authMethod, setAuthMethod] = useState<AuthMethod>(initialDraft.authMethod);
  const [privateKeyPath, setPrivateKeyPath] = useState(initialDraft.privateKeyPath ?? '');
  const [hostKeyPolicy, setHostKeyPolicy] = useState<HostKeyPolicy>(initialDraft.hostKeyPolicy);
  const [saveAsSession, setSaveAsSession] = useState(mode === 'edit' ? true : initialDraft.saveAsSession);
  const [connecting, setConnecting] = useState(false);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setConnecting(true);
    try {
      await onConnect({
        protocol,
        name: protocol === 'local' ? name || '本地终端' : name || `${username || 'user'}@${host}`,
        host,
        port,
        username,
        authMethod,
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
      <form className="dialog quick-connect" onSubmit={submit}>
        <header>
          <span className="eyebrow">{mode === 'edit' ? 'Session Editor' : 'Quick Connect'}</span>
          <h2>{mode === 'edit' ? '编辑会话' : '快速连接'}</h2>
          <p>阶段 2 使用系统 OpenSSH 打开远程 shell；密码和私钥口令在终端提示中输入，不写入普通配置或导出文件。</p>
        </header>
        <div className="form-grid">
          <label>
            名称
            <input value={name} onChange={(event) => setName(event.target.value)} placeholder="生产跳板机" />
          </label>
          <label>
            协议
            <select value={protocol} onChange={(event) => setProtocol(event.target.value as SessionProtocol)}>
              <option value="ssh">SSH</option>
              <option value="local">本地 Shell</option>
            </select>
          </label>
        </div>
        {protocol === 'ssh' && (
          <>
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
                认证方式
                <select value={authMethod} onChange={(event) => setAuthMethod(event.target.value as AuthMethod)}>
                  <option value="password">密码</option>
                  <option value="private_key">私钥</option>
                  <option value="agent">SSH Agent</option>
                </select>
              </label>
              {authMethod === 'private_key' && (
                <>
                  <label>
                    私钥路径
                    <input value={privateKeyPath} onChange={(event) => setPrivateKeyPath(event.target.value)} placeholder="~/.ssh/id_ed25519" />
                  </label>
                </>
              )}
              <label>
                主机密钥策略
                <select value={hostKeyPolicy} onChange={(event) => setHostKeyPolicy(event.target.value as HostKeyPolicy)}>
                  <option value="prompt">未知或变化时提示</option>
                  <option value="accept_new">自动信任新主机</option>
                  <option value="strict">仅连接已信任主机</option>
                </select>
              </label>
            </div>
            <p className="security-note">阶段 2 验收项：未知主机密钥、密钥变化和凭据输入均由系统 OpenSSH 在终端会话内显式处理。</p>
          </>
        )}
        {mode === 'connect' && (
          <label className="checkbox-row">
            <input type="checkbox" checked={saveAsSession} onChange={(event) => setSaveAsSession(event.target.checked)} />
            保存为会话配置（不保存密码或口令）
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
    authMethod: session.auth.method,
    privateKeyPath: session.auth.privateKeyPath ?? '',
    hostKeyPolicy: 'prompt',
    saveAsSession: true,
  };
}
