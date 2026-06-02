import type { TerminalConfig, WorkspaceTab } from '../../bindings/types';
import { TerminalPane } from '../terminal/TerminalPane';

interface WorkspaceProps {
  tabs: WorkspaceTab[];
  activeTabId: string;
  terminalConfig: TerminalConfig;
  onActivateTab: (tabId: string) => void;
  onCloseTab: (tabId: string) => void;
}

export function Workspace({ tabs, activeTabId, terminalConfig, onActivateTab, onCloseTab }: WorkspaceProps) {
  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? tabs[0];

  return (
    <section className="workspace" aria-label="终端工作区">
      <div className="tab-strip" role="tablist">
        {tabs.map((tab) => (
          <button
            className="tab"
            data-active={tab.id === activeTabId}
            key={tab.id}
            role="tab"
            type="button"
            onClick={() => onActivateTab(tab.id)}
          >
            <span>{tab.title}</span>
            <span className="tab-status">{tab.panes.some((pane) => pane.status === 'connected') ? '●' : '○'}</span>
            <span
              className="tab-close"
              role="button"
              tabIndex={0}
              onClick={(event) => {
                event.stopPropagation();
                onCloseTab(tab.id);
              }}
            >
              ×
            </span>
          </button>
        ))}
      </div>
      <div className="pane-grid">
        {activeTab.panes.map((pane) => (
          <TerminalPane config={terminalConfig} key={pane.id} pane={pane} active={pane.id === activeTab.activePaneId} />
        ))}
      </div>
    </section>
  );
}
