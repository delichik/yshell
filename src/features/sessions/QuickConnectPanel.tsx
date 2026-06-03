import { FormEvent, useState } from 'react';
import type { AuthMethod, QuickConnectDraft, SessionProtocol } from '../../bindings/types';

interface QuickConnectPanelProps {
  onCancel: () => void;
  onConnect: (draft: QuickConnectDraft) => Promise<void>;
}

export function QuickConnectPanel({ onCancel, onConnect }: QuickConnectPanelProps) {
  const [protocol, setProtocol] = useState<SessionProtocol>('ssh');
  const [host, setHost] = useState('');
  const [port, setPort] = useState(22);
  const [username, setUsername] = useState('');
  const [authMethod, setAuthMethod] = useState<AuthMethod>('password');
  const [saveAsSession, setSaveAsSession] = useState(true);
  const [connecting, setConnecting] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setConnecting(true);
    setErrorMessage(null);
    try {
      await onConnect({
        protocol,
        name: protocol === 'local' ? '本地终端' : `${username || 'user'}@${host}`,
        host,
        port,
        username,
        authMethod,
        saveAsSession,
      });
    } catch (error) {
      setErrorMessage(error instanceof Error ? error.message : '连接失败，请检查连接参数后重试。');
      setConnecting(false);
    }
  };

  return (
    <div className="dialog-backdrop" role="presentation">
      <form className="dialog quick-connect" onSubmit={submit}>
        <header>
          <span className="eyebrow">Quick Connect</span>
          <h2>快速连接</h2>
          <p>输入连接信息后立即打开标签页；敏感凭据将由后端安全存储处理。</p>
        </header>
        <label>
          协议
          <select value={protocol} onChange={(event) => setProtocol(event.target.value as SessionProtocol)}>
            <option value="ssh">SSH</option>
            <option value="local">本地 Shell</option>
          </select>
        </label>
        {protocol === 'ssh' && (
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
          </div>
        )}
        <label className="checkbox-row">
          <input type="checkbox" checked={saveAsSession} onChange={(event) => setSaveAsSession(event.target.checked)} />
          保存为会话配置
        </label>
        {errorMessage && <p className="form-error" role="alert">{errorMessage}</p>}
        <footer>
          <button type="button" onClick={onCancel}>取消</button>
          <button type="submit" className="primary" disabled={connecting}>{connecting ? '连接中…' : '连接'}</button>
        </footer>
      </form>
    </div>
  );
}
