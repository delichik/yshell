import { useState } from 'react';
import type { AppSettings } from '../../bindings/types';

interface SettingsPanelProps {
  settings: AppSettings;
  onClose: () => void;
  onSave: (settings: AppSettings) => Promise<void>;
}

export function SettingsPanel({ settings, onClose, onSave }: SettingsPanelProps) {
  const [draft, setDraft] = useState<AppSettings>(settings);

  return (
    <div className="dialog-backdrop" role="presentation">
      <section className="dialog settings-dialog">
        <header>
          <span className="eyebrow">Settings</span>
          <h2>设置中心</h2>
          <p>当前实现覆盖常规、终端、主题、连接、安全与日志的工程基线。</p>
        </header>
        <div className="settings-grid">
          <label>
            应用主题
            <select
              value={draft.appearance.appTheme}
              onChange={(event) =>
                setDraft({ ...draft, appearance: { ...draft.appearance, appTheme: event.target.value as AppSettings['appearance']['appTheme'] } })
              }
            >
              <option value="system">跟随系统</option>
              <option value="dark">深色</option>
              <option value="light">浅色</option>
            </select>
          </label>
          <label>
            字体
            <input
              value={draft.terminal.fontFamily}
              onChange={(event) => setDraft({ ...draft, terminal: { ...draft.terminal, fontFamily: event.target.value } })}
            />
          </label>
          <label>
            字号
            <input
              type="number"
              value={draft.terminal.fontSize}
              onChange={(event) => setDraft({ ...draft, terminal: { ...draft.terminal, fontSize: Number(event.target.value) } })}
            />
          </label>
          <label className="checkbox-row">
            <input
              type="checkbox"
              checked={draft.logging.enabled}
              onChange={(event) => setDraft({ ...draft, logging: { ...draft.logging, enabled: event.target.checked } })}
            />
            默认启用会话日志提示
          </label>
        </div>
        <footer>
          <button type="button" onClick={onClose}>取消</button>
          <button type="button" className="primary" onClick={() => void onSave(draft)}>保存</button>
        </footer>
      </section>
    </div>
  );
}
