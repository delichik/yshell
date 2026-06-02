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
              min={8}
              max={48}
              value={draft.terminal.fontSize}
              onChange={(event) => setDraft({ ...draft, terminal: { ...draft.terminal, fontSize: Number(event.target.value) } })}
            />
          </label>
          <label>
            行高
            <input
              type="number"
              min={1}
              max={2}
              step={0.05}
              value={draft.terminal.lineHeight}
              onChange={(event) => setDraft({ ...draft, terminal: { ...draft.terminal, lineHeight: Number(event.target.value) } })}
            />
          </label>
          <label>
            滚动缓冲
            <input
              type="number"
              min={100}
              step={100}
              value={draft.terminal.scrollback}
              onChange={(event) => setDraft({ ...draft, terminal: { ...draft.terminal, scrollback: Number(event.target.value) } })}
            />
          </label>
          <label>
            配色方案
            <select
              value={draft.terminal.colorScheme}
              onChange={(event) => setDraft({ ...draft, terminal: { ...draft.terminal, colorScheme: event.target.value } })}
            >
              <option value="One Dark">One Dark</option>
              <option value="Light">Light</option>
              <option value="Solarized Light">Solarized Light</option>
            </select>
          </label>
          <label>
            默认 Shell
            <input
              value={draft.terminal.shell ?? ''}
              onChange={(event) => setDraft({ ...draft, terminal: { ...draft.terminal, shell: event.target.value || undefined } })}
              placeholder="留空使用系统默认"
            />
          </label>
          <label>
            工作目录
            <input
              value={draft.terminal.workingDirectory ?? ''}
              onChange={(event) => setDraft({ ...draft, terminal: { ...draft.terminal, workingDirectory: event.target.value || undefined } })}
              placeholder="留空使用当前目录"
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
