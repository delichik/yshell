import type { AppSettings } from '../../bindings/types';

interface SettingsPanelProps {
  settings: AppSettings;
  onClose: () => void;
  onChange: (settings: AppSettings) => void;
}

export function SettingsPanel({ settings, onClose, onChange }: SettingsPanelProps) {
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
              value={settings.appearance.appTheme}
              onChange={(event) =>
                onChange({ ...settings, appearance: { ...settings.appearance, appTheme: event.target.value as AppSettings['appearance']['appTheme'] } })
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
              value={settings.terminal.fontFamily}
              onChange={(event) => onChange({ ...settings, terminal: { ...settings.terminal, fontFamily: event.target.value } })}
            />
          </label>
          <label>
            字号
            <input
              type="number"
              value={settings.terminal.fontSize}
              onChange={(event) => onChange({ ...settings, terminal: { ...settings.terminal, fontSize: Number(event.target.value) } })}
            />
          </label>
          <label className="checkbox-row">
            <input
              type="checkbox"
              checked={settings.logging.enabled}
              onChange={(event) => onChange({ ...settings, logging: { ...settings.logging, enabled: event.target.checked } })}
            />
            默认启用会话日志提示
          </label>
        </div>
        <footer>
          <button type="button" className="primary" onClick={onClose}>完成</button>
        </footer>
      </section>
    </div>
  );
}
