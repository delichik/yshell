import { useEffect, useMemo, useRef, useState } from 'react';
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
  type TerminalConfig,
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

function sessionToQuickConnectDraft(session: SessionProfile): QuickConnectDraft {
  return {
    protocol: session.protocol,
    name: session.name,
    host: session.host ?? '',
    port: session.port ?? 22,
    username: session.username ?? session.auth.username ?? '',
    authMethod: session.auth.method,
    saveAsSession: false,
  };
}

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
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const startupTerminalOpenedRef = useRef(false);

  useEffect(() => {
    void listSessions().then(setSessions).catch((error) => console.error('Failed to load sessions', error));
    void loadSettings()
      .then((storedSettings) => {
        if (storedSettings) setSettings(storedSettings);
      })
      .catch((error) => console.error('Failed to load settings', error))
      .finally(() => setSettingsLoaded(true));
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

  const updatePaneRuntime = (tabId: string, paneId: string, runtimeId: string | null, title: string, status: RuntimeStatus) => {
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

  const openLocal = async (terminalConfig: TerminalConfig = settings.terminal, tabTitle = '正在打开本地终端') => {
    const tab = createEmptyTab(tabs.length + 1);
    tab.activePaneId = tab.panes[0].id;
    setTabs((current) => [...current, { ...tab, title: tabTitle }]);
    setActiveTabId(tab.id);
    try {
      const runtime = await openLocalTerminal(tab.id, tab.panes[0].id, 120, 30, terminalConfig);
      updatePaneRuntime(tab.id, tab.panes[0].id, runtime.runtimeId, runtime.title, runtime.status);
    } catch (error) {
      console.error('Failed to open local terminal', error);
      updatePaneRuntime(tab.id, tab.panes[0].id, null, '本地终端启动失败', 'failed');
      throw error;
    }
  };

  useEffect(() => {
    if (!settingsLoaded || startupTerminalOpenedRef.current) return;
    const initialTab = tabs[0];
    const initialPane = initialTab?.panes[0];
    if (!initialTab || !initialPane || initialPane.runtimeId || initialPane.status !== 'idle') return;

    startupTerminalOpenedRef.current = true;
    setTabs((current) =>
      current.map((tab) =>
        tab.id === initialTab.id
          ? {
              ...tab,
              title: '正在打开本地终端',
              panes: tab.panes.map((pane) =>
                pane.id === initialPane.id ? { ...pane, title: '正在打开本地终端', status: 'connecting' } : pane,
              ),
            }
          : tab,
      ),
    );
    void openLocalTerminal(initialTab.id, initialPane.id, 120, 30, settings.terminal)
      .then((runtime) => updatePaneRuntime(initialTab.id, initialPane.id, runtime.runtimeId, runtime.title, runtime.status))
      .catch((error) => {
        console.error('Failed to open startup local terminal', error);
        updatePaneRuntime(initialTab.id, initialPane.id, null, '本地终端启动失败', 'failed');
      });
  }, [settingsLoaded, settings.terminal, tabs]);

  const openQuickConnection = async (draft: QuickConnectDraft) => {
    const tab = createEmptyTab(tabs.length + 1);
    tab.activePaneId = tab.panes[0].id;
    const title = draft.protocol === 'local' ? '本地终端' : draft.name || `${draft.username}@${draft.host}`;
    setTabs((current) => [...current, { ...tab, title }]);
    setActiveTabId(tab.id);
    try {
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
      setQuickConnectOpen(false);
    } catch (error) {
      console.error('Failed to open quick connection', error);
      updatePaneRuntime(tab.id, tab.panes[0].id, null, '连接失败', 'failed');
      throw error;
    }
  };

  const openSavedSession = async (session: SessionProfile) => {
    if (session.protocol === 'local') {
      await openLocal(session.terminal, session.name || '本地终端');
      return;
    }
    await openQuickConnection(sessionToQuickConnectDraft(session));
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
          <button type="button" onClick={() => void openLocal()}>新建本地终端</button>
          <button type="button" onClick={() => setSettingsOpen(true)}>设置</button>
        </nav>
      </header>
      <main className="main-layout">
        <SessionSidebar
          sessions={sessions}
          onOpenLocal={() => void openLocal()}
          onOpenSession={(session) => void openSavedSession(session)}
          onQuickConnect={() => setQuickConnectOpen(true)}
        />
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
