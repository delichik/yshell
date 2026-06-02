import { useEffect, useMemo, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { SessionSidebar } from '../features/sessions/SessionSidebar';
import { QuickConnectPanel } from '../features/sessions/QuickConnectPanel';
import { Workspace } from '../features/workspace/Workspace';
import { SettingsPanel } from '../features/settings/SettingsPanel';
import { StatusBar } from '../components/StatusBar';
import { closeAllTerminals, closeTerminal, listSessions, loadSettings, openLocalTerminal, openSshTerminal, runningInTauri, saveSession, saveSettings } from '../bindings/ipc';
import {
  defaultAppearance,
  defaultLogging,
  defaultTerminalConfig,
  type AppSettings,
  type QuickConnectDraft,
  type RuntimeStatus,
  type SessionProfile,
  type TerminalStatusEvent,
  type WorkspaceTab,
} from '../bindings/types';

const initialSettings: AppSettings = {
  appearance: defaultAppearance,
  terminal: defaultTerminalConfig,
  logging: defaultLogging,
};

const createEmptyTab = (index = 1): WorkspaceTab => ({
  id: crypto.randomUUID(),
  title: `本地终端 ${index}`,
  locked: false,
  panes: [
    {
      id: crypto.randomUUID(),
      runtimeId: null,
      title: '未连接',
      status: 'idle',
    },
  ],
  activePaneId: '',
});

export function App() {
  const [sessions, setSessions] = useState<SessionProfile[]>([]);
  const [settings, setSettings] = useState<AppSettings>(initialSettings);
  const [tabs, setTabs] = useState<WorkspaceTab[]>(() => {
    const tab = createEmptyTab();
    return [{ ...tab, activePaneId: tab.panes[0].id }];
  });
  const [activeTabId, setActiveTabId] = useState(() => tabs[0].id);
  const [quickConnectOpen, setQuickConnectOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    void listSessions().then(setSessions).catch((error) => console.error('Failed to load sessions', error));
    void loadSettings()
      .then((storedSettings) => storedSettings && setSettings(storedSettings))
      .catch((error) => console.error('Failed to load settings', error));
  }, []);

  useEffect(() => {
    if (!runningInTauri) return undefined;
    let unlisten: (() => void) | undefined;
    void listen<TerminalStatusEvent>('terminal://status', (event) => {
      updateRuntimeStatus(event.payload.runtimeId, event.payload.status);
    }).then((cleanup) => {
      unlisten = cleanup;
    });

    const closeAll = () => {
      void closeAllTerminals();
    };
    window.addEventListener('beforeunload', closeAll);

    return () => {
      unlisten?.();
      window.removeEventListener('beforeunload', closeAll);
      closeAll();
    };
  }, []);

  const activeTab = useMemo(() => tabs.find((tab) => tab.id === activeTabId) ?? tabs[0], [activeTabId, tabs]);
  const activePane = activeTab?.panes.find((pane) => pane.id === activeTab.activePaneId) ?? activeTab?.panes[0];

  const updateRuntimeStatus = (runtimeId: string, status: RuntimeStatus) => {
    setTabs((current) =>
      current.map((tab) => ({
        ...tab,
        panes: tab.panes.map((pane) => (pane.runtimeId === runtimeId ? { ...pane, status } : pane)),
      })),
    );
  };

  const updatePaneRuntime = (tabId: string, paneId: string, runtimeId: string, title: string, status: RuntimeStatus) => {
    setTabs((current) =>
      current.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              title,
              panes: tab.panes.map((pane) => (pane.id === paneId ? { ...pane, runtimeId, title, status } : pane)),
            }
          : tab,
      ),
    );
  };

  const openLocal = async () => {
    const tab = createEmptyTab(tabs.length + 1);
    tab.activePaneId = tab.panes[0].id;
    setTabs((current) => [...current, { ...tab, title: '正在打开本地终端' }]);
    setActiveTabId(tab.id);
    const runtime = await openLocalTerminal(tab.id, tab.panes[0].id, 120, 30, settings.terminal);
    updatePaneRuntime(tab.id, tab.panes[0].id, runtime.runtimeId, runtime.title, runtime.status);
  };

  const openQuickConnection = async (draft: QuickConnectDraft) => {
    const tab = createEmptyTab(tabs.length + 1);
    tab.activePaneId = tab.panes[0].id;
    const title = draft.protocol === 'local' ? '本地终端' : draft.name || `${draft.username}@${draft.host}`;
    setTabs((current) => [...current, { ...tab, title }]);
    setActiveTabId(tab.id);
    setQuickConnectOpen(false);
    if (draft.saveAsSession) {
      const now = new Date().toISOString();
      const stored = await saveSession({
        id: crypto.randomUUID(),
        name: title,
        folderId: null,
        tags: [],
        protocol: draft.protocol,
        host: draft.protocol === 'ssh' ? draft.host : null,
        port: draft.protocol === 'ssh' ? draft.port : null,
        username: draft.protocol === 'ssh' ? draft.username || null : null,
        auth: { method: draft.authMethod, username: draft.username || undefined },
        proxy: null,
        terminal: settings.terminal,
        appearance: settings.appearance,
        logging: settings.logging,
        createdAt: now,
        updatedAt: now,
        lastConnectedAt: null,
      });
      setSessions((current) => [stored, ...current.filter((session) => session.id !== stored.id)]);
    }
    const runtime =
      draft.protocol === 'local'
        ? await openLocalTerminal(tab.id, tab.panes[0].id, 120, 30, settings.terminal)
        : await openSshTerminal(draft, tab.id, tab.panes[0].id);
    updatePaneRuntime(tab.id, tab.panes[0].id, runtime.runtimeId, runtime.title, runtime.status);
  };

  const persistSettings = async (nextSettings: AppSettings) => {
    const storedSettings = await saveSettings(nextSettings);
    setSettings(storedSettings);
    setSettingsOpen(false);
  };

  const closeTab = (tabId: string) => {
    const closingTab = tabs.find((tab) => tab.id === tabId);
    closingTab?.panes.forEach((pane) => {
      if (pane.runtimeId) {
        void closeTerminal(pane.runtimeId);
      }
    });

    setTabs((current) => {
      const next = current.filter((tab) => tab.id !== tabId);
      if (next.length === 0) {
        const replacement = createEmptyTab();
        replacement.activePaneId = replacement.panes[0].id;
        setActiveTabId(replacement.id);
        return [replacement];
      }
      if (activeTabId === tabId) setActiveTabId(next[0].id);
      return next;
    });
  };

  return (
    <div className="app-shell" data-theme={settings.appearance.appTheme}>
      <header className="title-bar">
        <div>
          <strong>YShell</strong>
          <span>开源跨平台终端与远程会话客户端</span>
        </div>
        <nav aria-label="主操作">
          <button type="button" onClick={() => setQuickConnectOpen(true)}>快速连接</button>
          <button type="button" onClick={openLocal}>新建本地终端</button>
          <button type="button" onClick={() => setSettingsOpen(true)}>设置</button>
        </nav>
      </header>
      <main className="main-layout">
        <SessionSidebar sessions={sessions} onQuickConnect={() => setQuickConnectOpen(true)} />
        <Workspace
          tabs={tabs}
          activeTabId={activeTabId}
          terminalConfig={settings.terminal}
          onActivateTab={setActiveTabId}
          onCloseTab={closeTab}
        />
      </main>
      <StatusBar pane={activePane} loggingEnabled={settings.logging.enabled} />
      {quickConnectOpen && <QuickConnectPanel onCancel={() => setQuickConnectOpen(false)} onConnect={openQuickConnection} />}
      {settingsOpen && (
        <SettingsPanel settings={settings} onClose={() => setSettingsOpen(false)} onSave={persistSettings} />
      )}
    </div>
  );
}
