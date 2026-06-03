import { ChangeEvent, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { SessionSidebar } from '../features/sessions/SessionSidebar';
import { QuickConnectPanel } from '../features/sessions/QuickConnectPanel';
import { Workspace } from '../features/workspace/Workspace';
import { SettingsPanel } from '../features/settings/SettingsPanel';
import { StatusBar } from '../components/StatusBar';
import {
  closeAllTerminals,
  closeTerminal,
  deleteSession,
  duplicateSession,
  exportSessions,
  importSessions,
  listSessions,
  loadSettings,
  openLocalTerminal,
  openSshTerminal,
  runningInTauri,
  saveSession,
  saveSettings,
} from '../bindings/ipc';
import {
  defaultAppearance,
  defaultLogging,
  defaultTerminalConfig,
  type AppSettings,
  type QuickConnectDraft,
  type RuntimeStatus,
  type SessionExportBundle,
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
  const [editingSession, setEditingSession] = useState<SessionProfile | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [sessionMessage, setSessionMessage] = useState<string | null>(null);
  const importInputRef = useRef<HTMLInputElement | null>(null);

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

  const openLocal = async () => {
    const tab = createEmptyTab(tabs.length + 1);
    tab.activePaneId = tab.panes[0].id;
    setTabs((current) => [...current, { ...tab, title: '正在打开本地终端' }]);
    setActiveTabId(tab.id);
    const runtime = await openLocalTerminal(tab.id, tab.panes[0].id, 120, 30, settings.terminal);
    updatePaneRuntime(tab.id, tab.panes[0].id, runtime.runtimeId, runtime.title, runtime.status);
  };

  const profileFromDraft = (draft: QuickConnectDraft, existing?: SessionProfile | null): SessionProfile => {
    const now = new Date().toISOString();
    const title = draft.protocol === 'local' ? draft.name || '本地终端' : draft.name || `${draft.username || 'user'}@${draft.host}`;
    return {
      id: existing?.id ?? crypto.randomUUID(),
      name: title,
      folderId: existing?.folderId ?? null,
      tags: existing?.tags ?? [],
      protocol: draft.protocol,
      host: draft.protocol === 'ssh' ? draft.host : null,
      port: draft.protocol === 'ssh' ? draft.port : null,
      username: draft.protocol === 'ssh' ? draft.username || null : null,
      auth: {
        method: draft.authMethod,
        username: draft.username || undefined,
        privateKeyPath: draft.authMethod === 'private_key' ? draft.privateKeyPath || undefined : undefined,
        credentialRef: existing?.auth.credentialRef,
      },
      proxy: existing?.proxy ?? null,
      terminal: existing?.terminal ?? settings.terminal,
      appearance: existing?.appearance ?? settings.appearance,
      logging: existing?.logging ?? settings.logging,
      createdAt: existing?.createdAt ?? now,
      updatedAt: now,
      lastConnectedAt: existing?.lastConnectedAt ?? null,
    };
  };

  const openQuickConnection = async (draft: QuickConnectDraft) => {
    const tab = createEmptyTab(tabs.length + 1);
    tab.activePaneId = tab.panes[0].id;
    const title = draft.protocol === 'local' ? draft.name || '本地终端' : draft.name || `${draft.username}@${draft.host}`;
    setTabs((current) => [...current, { ...tab, title }]);
    setActiveTabId(tab.id);

    try {
      const runtime =
        draft.protocol === 'local'
          ? await openLocalTerminal(tab.id, tab.panes[0].id, 120, 30, settings.terminal)
          : await openSshTerminal(draft, tab.id, tab.panes[0].id);
      updatePaneRuntime(tab.id, tab.panes[0].id, runtime.runtimeId, runtime.title, runtime.status);
      setQuickConnectOpen(false);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      updatePaneRuntime(tab.id, tab.panes[0].id, null, '连接失败', 'failed');
      setSessionMessage(`连接失败：${message}`);
      return;
    }

    if (!draft.saveAsSession) return;
    try {
      const stored = await saveSession(profileFromDraft(draft));
      setSessions((current) => [stored, ...current.filter((session) => session.id !== stored.id)]);
      setSessionMessage(`已连接并保存会话：${stored.name}`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setSessionMessage(`连接已打开，但保存会话失败：${message}`);
    }
  };

  const openSavedSession = async (session: SessionProfile) => {
    if (session.protocol === 'local') {
      await openLocal();
      return;
    }
    await openQuickConnection({
      protocol: 'ssh',
      name: session.name,
      host: session.host ?? '',
      port: session.port ?? 22,
      username: session.username ?? session.auth.username ?? '',
      authMethod: session.auth.method,
      privateKeyPath: session.auth.privateKeyPath,
      hostKeyPolicy: 'prompt',
      saveAsSession: false,
    });
  };

  const saveEditedSession = async (draft: QuickConnectDraft) => {
    if (!editingSession) return;
    const stored = await saveSession(profileFromDraft(draft, editingSession));
    setSessions((current) => [stored, ...current.filter((session) => session.id !== stored.id)]);
    setEditingSession(null);
    setSessionMessage(`已更新会话：${stored.name}`);
  };

  const removeSession = async (session: SessionProfile) => {
    if (!window.confirm(`确定删除会话“${session.name}”吗？凭据引用不会随普通配置自动删除。`)) return;
    const nextSessions = await deleteSession(session.id);
    setSessions(nextSessions);
    setSessionMessage(`已删除会话：${session.name}`);
  };

  const copySession = async (session: SessionProfile) => {
    const cloned = await duplicateSession(session.id);
    setSessions((current) => [cloned, ...current]);
    setSessionMessage(`已复制会话：${cloned.name}`);
  };

  const exportSessionFile = async () => {
    const bundle = await exportSessions();
    const blob = new Blob([JSON.stringify(bundle, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = `yshell-sessions-${new Date().toISOString().slice(0, 10)}.json`;
    anchor.click();
    URL.revokeObjectURL(url);
    setSessionMessage('已导出会话配置（不包含密码或口令）。');
  };

  const importSessionFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    try {
      const bundle = JSON.parse(await file.text()) as SessionExportBundle;
      const nextSessions = await importSessions(bundle);
      setSessions(nextSessions);
      setSessionMessage(`已导入 ${nextSessions.length} 个会话。`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setSessionMessage(`导入失败：${message}`);
    }
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
          <button type="button" onClick={() => void exportSessionFile()}>导出会话</button>
          <button type="button" onClick={() => importInputRef.current?.click()}>导入会话</button>
          <button type="button" onClick={() => setSettingsOpen(true)}>设置</button>
        </nav>
        <input ref={importInputRef} className="visually-hidden" type="file" accept="application/json" onChange={(event) => void importSessionFile(event)} />
      </header>
      <main className="main-layout">
        <SessionSidebar
          sessions={sessions}
          onQuickConnect={() => setQuickConnectOpen(true)}
          onOpenLocal={openLocal}
          onOpenSession={(session) => void openSavedSession(session)}
          onEditSession={setEditingSession}
          onDuplicateSession={(session) => void copySession(session)}
          onDeleteSession={(session) => void removeSession(session)}
        />
        <Workspace
          tabs={tabs}
          activeTabId={activeTabId}
          terminalConfig={settings.terminal}
          onActivateTab={setActiveTabId}
          onCloseTab={closeTab}
        />
      </main>
      <StatusBar pane={activePane} loggingEnabled={settings.logging.enabled} message={sessionMessage} />
      {quickConnectOpen && <QuickConnectPanel onCancel={() => setQuickConnectOpen(false)} onConnect={openQuickConnection} />}
      {editingSession && (
        <QuickConnectPanel
          mode="edit"
          initialSession={editingSession}
          onCancel={() => setEditingSession(null)}
          onConnect={saveEditedSession}
        />
      )}
      {settingsOpen && (
        <SettingsPanel settings={settings} onClose={() => setSettingsOpen(false)} onSave={persistSettings} />
      )}
    </div>
  );
}
