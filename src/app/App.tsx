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
  type SplitDirection,
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

const createEmptyTab = (index = 1): WorkspaceTab => {
  const paneId = crypto.randomUUID();
  return {
    id: crypto.randomUUID(),
    title: `本地终端 ${index}`,
    locked: false,
    panes: [
      {
        id: paneId,
        runtimeId: null,
        title: '未连接',
        status: 'idle',
      },
    ],
    activePaneId: paneId,
    splitDirection: 'vertical',
    splitRatio: 50,
  };
};

const workspaceStorageKey = 'yshell.workspace.tabs.v1';

const restoreWorkspaceTabs = (): WorkspaceTab[] | null => {
  if (typeof window === 'undefined') return null;
  const stored = window.localStorage.getItem(workspaceStorageKey);
  if (!stored) return null;
  try {
    const restored = JSON.parse(stored) as WorkspaceTab[];
    if (!Array.isArray(restored) || restored.length === 0) return null;
    return restored.map((tab, tabIndex) => {
      const panes = Array.isArray(tab.panes) && tab.panes.length > 0 ? tab.panes : createEmptyTab(tabIndex + 1).panes;
      return {
        ...tab,
        locked: Boolean(tab.locked),
        panes: panes.map((pane) => ({ ...pane, runtimeId: null, status: 'idle' as RuntimeStatus })),
        activePaneId: panes.some((pane) => pane.id === tab.activePaneId) ? tab.activePaneId : panes[0].id,
        splitDirection: tab.splitDirection ?? 'vertical',
        splitRatio: typeof tab.splitRatio === 'number' ? tab.splitRatio : 50,
      };
    });
  } catch {
    return null;
  }
};

export function App() {
  const [sessions, setSessions] = useState<SessionProfile[]>([]);
  const [settings, setSettings] = useState<AppSettings>(initialSettings);
  const [tabs, setTabs] = useState<WorkspaceTab[]>(() => restoreWorkspaceTabs() ?? [createEmptyTab()]);
  const [activeTabId, setActiveTabId] = useState(() => tabs[0].id);
  const [quickConnectOpen, setQuickConnectOpen] = useState(false);
  const [quickConnectTarget, setQuickConnectTarget] = useState<{ tabId: string; paneId: string } | null>(null);
  const [editingSession, setEditingSession] = useState<SessionProfile | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [sessionMessage, setSessionMessage] = useState<string | null>(null);
  const [broadcastEnabled, setBroadcastEnabled] = useState(false);
  const [broadcastTargetPaneIds, setBroadcastTargetPaneIds] = useState<string[]>([]);
  const [renamingTabId, setRenamingTabId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState('');
  const importInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    void listSessions().then(setSessions).catch((error) => console.error('Failed to load sessions', error));
    void loadSettings()
      .then((storedSettings) => storedSettings && setSettings(storedSettings))
      .catch((error) => console.error('Failed to load settings', error));
  }, []);

  useEffect(() => {
    const serializableTabs = tabs.map((tab) => ({
      ...tab,
      panes: tab.panes.map((pane) => ({ ...pane, runtimeId: null, status: 'idle' as RuntimeStatus })),
    }));
    if (typeof window !== 'undefined') window.localStorage.setItem(workspaceStorageKey, JSON.stringify(serializableTabs));
  }, [tabs]);

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
  const activeBroadcastTargetCount = broadcastEnabled
    ? activeTab.panes.filter((pane) => pane.runtimeId && broadcastTargetPaneIds.includes(pane.id)).length
    : 0;

  const updateRuntimeStatus = (runtimeId: string, status: RuntimeStatus) => {
    setTabs((current) =>
      current.map((tab) => ({
        ...tab,
        panes: tab.panes.map((pane) => (pane.runtimeId === runtimeId ? { ...pane, status } : pane)),
      })),
    );
  };

  const updatePaneRuntime = (tabId: string, paneId: string, runtimeId: string | null, title: string, status: RuntimeStatus, updateTabTitle = true) => {
    setTabs((current) =>
      current.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              title: updateTabTitle ? title : tab.title,
              panes: tab.panes.map((pane) => (pane.id === paneId ? { ...pane, runtimeId, title, status } : pane)),
            }
          : tab,
      ),
    );
  };

  const openLocalInPane = async (tabId: string, paneId: string, updateTabTitle = false) => {
    updatePaneRuntime(tabId, paneId, null, '正在打开本地终端', 'connecting', updateTabTitle);
    try {
      const runtime = await openLocalTerminal(tabId, paneId, 120, 30, settings.terminal);
      updatePaneRuntime(tabId, paneId, runtime.runtimeId, runtime.title, runtime.status, updateTabTitle);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      updatePaneRuntime(tabId, paneId, null, '本地终端打开失败', 'failed', updateTabTitle);
      setSessionMessage(`本地终端打开失败：${message}`);
    }
  };

  const openLocal = async () => {
    const tab = createEmptyTab(tabs.length + 1);
    setTabs((current) => [...current, { ...tab, title: '正在打开本地终端' }]);
    setActiveTabId(tab.id);
    await openLocalInPane(tab.id, tab.panes[0].id, true);
  };

  const profileFromDraft = (draft: QuickConnectDraft, existing?: SessionProfile | null): SessionProfile => {
    const now = new Date().toISOString();
    const title = draft.protocol === 'local' ? draft.name || '本地终端' : draft.name || `${draft.username || 'user'}@${draft.host}`;
    return {
      id: existing?.id ?? crypto.randomUUID(),
      name: title,
      folderId: draft.folderId === undefined ? existing?.folderId ?? null : draft.folderId,
      tags: draft.tags ?? existing?.tags ?? [],
      description: draft.description,
      color: draft.color,
      icon: existing?.icon,
      favorite: Boolean(draft.favorite),
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
    const target = quickConnectTarget;
    const tab = target ? tabs.find((item) => item.id === target.tabId) : createEmptyTab(tabs.length + 1);
    if (!tab) return;
    const paneId = target ? target.paneId : tab.panes[0].id;
    const title = draft.protocol === 'local' ? draft.name || '本地终端' : draft.name || `${draft.username}@${draft.host}`;
    if (!target) {
      setTabs((current) => [...current, { ...tab, title }]);
      setActiveTabId(tab.id);
    } else {
      setActiveTabId(target.tabId);
      activatePane(target.tabId, target.paneId);
    }

    try {
      updatePaneRuntime(tab.id, paneId, null, `正在连接 ${title}`, 'connecting', !target);
      const runtime =
        draft.protocol === 'local'
          ? await openLocalTerminal(tab.id, paneId, 120, 30, settings.terminal)
          : await openSshTerminal(draft, tab.id, paneId);
      updatePaneRuntime(tab.id, paneId, runtime.runtimeId, runtime.title, runtime.status, !target);
      setQuickConnectOpen(false);
      setQuickConnectTarget(null);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      updatePaneRuntime(tab.id, paneId, null, '连接失败', 'failed', !target);
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
      description: session.description,
      tags: session.tags,
      folderId: session.folderId,
      color: session.color,
      favorite: session.favorite,
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

  const hasActiveConnection = (tab: WorkspaceTab) =>
    tab.panes.some((pane) => pane.runtimeId && pane.status !== 'failed' && pane.status !== 'disconnected');

  const closeTab = (tabId: string) => {
    const closingTab = tabs.find((tab) => tab.id === tabId);
    if (!closingTab) return;
    if (closingTab.locked && !window.confirm(`标签“${closingTab.title}”已锁定，仍要关闭吗？`)) return;
    if (hasActiveConnection(closingTab) && !window.confirm(`标签“${closingTab.title}”仍有活跃连接，关闭后会断开终端。确认关闭吗？`)) return;

    closingTab.panes.forEach((pane) => {
      if (pane.runtimeId) {
        void closeTerminal(pane.runtimeId);
      }
    });

    setTabs((current) => {
      const next = current.filter((tab) => tab.id !== tabId);
      if (next.length === 0) {
        const replacement = createEmptyTab();
        setActiveTabId(replacement.id);
        return [replacement];
      }
      if (activeTabId === tabId) setActiveTabId(next[0].id);
      return next;
    });
  };

  const closeOtherTabs = (tabId: string) => {
    const closeableTabs = tabs.filter((tab) => tab.id !== tabId && !tab.locked);
    const activeCount = closeableTabs.filter(hasActiveConnection).length;
    if (activeCount > 0 && !window.confirm(`将关闭 ${activeCount} 个包含活跃连接的未锁定标签，锁定标签会保留。确认继续吗？`)) return;
    closeableTabs.forEach((tab) => {
      tab.panes.forEach((pane) => {
        if (pane.runtimeId) void closeTerminal(pane.runtimeId);
      });
    });
    setTabs((current) => current.filter((tab) => tab.id === tabId || tab.locked));
    setActiveTabId(tabId);
  };

  const renameTab = (tabId: string) => {
    const tab = tabs.find((item) => item.id === tabId);
    if (!tab) return;
    setRenamingTabId(tabId);
    setRenameDraft(tab.title);
  };

  const saveTabRename = () => {
    if (!renamingTabId) return;
    const title = renameDraft.trim();
    if (!title) return;
    setTabs((current) => current.map((item) => (item.id === renamingTabId ? { ...item, title } : item)));
    setRenamingTabId(null);
    setRenameDraft('');
  };

  const toggleTabLock = (tabId: string) => {
    setTabs((current) => current.map((tab) => (tab.id === tabId ? { ...tab, locked: !tab.locked } : tab)));
  };

  const moveTab = (tabId: string, direction: -1 | 1) => {
    setTabs((current) => {
      const index = current.findIndex((tab) => tab.id === tabId);
      const nextIndex = index + direction;
      if (index < 0 || nextIndex < 0 || nextIndex >= current.length) return current;
      const next = [...current];
      [next[index], next[nextIndex]] = [next[nextIndex], next[index]];
      return next;
    });
  };

  const reorderTab = (draggedTabId: string, targetTabId: string) => {
    if (draggedTabId === targetTabId) return;
    setTabs((current) => {
      const draggedIndex = current.findIndex((tab) => tab.id === draggedTabId);
      const targetIndex = current.findIndex((tab) => tab.id === targetTabId);
      if (draggedIndex < 0 || targetIndex < 0) return current;
      const next = [...current];
      const [dragged] = next.splice(draggedIndex, 1);
      next.splice(targetIndex, 0, dragged);
      return next;
    });
  };

  const activatePane = (tabId: string, paneId: string) => {
    setTabs((current) => current.map((tab) => (tab.id === tabId ? { ...tab, activePaneId: paneId } : tab)));
  };

  const focusRelativePane = (direction: -1 | 1) => {
    const paneIndex = activeTab.panes.findIndex((pane) => pane.id === activeTab.activePaneId);
    if (paneIndex < 0 || activeTab.panes.length < 2) return;
    const nextPane = activeTab.panes[(paneIndex + direction + activeTab.panes.length) % activeTab.panes.length];
    activatePane(activeTab.id, nextPane.id);
    setSessionMessage(`已切换到窗格：${nextPane.title}`);
  };

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!event.altKey || !event.shiftKey || (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft')) return;
      event.preventDefault();
      focusRelativePane(event.key === 'ArrowRight' ? 1 : -1);
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [activeTab]);

  const toggleBroadcast = () => {
    if (broadcastEnabled) {
      setBroadcastEnabled(false);
      setBroadcastTargetPaneIds([]);
      setSessionMessage('广播输入已关闭：输入仅进入活动窗格。');
      return;
    }
    const runtimePaneIds = activeTab.panes.filter((pane) => pane.runtimeId).map((pane) => pane.id);
    if (runtimePaneIds.length < 2) {
      window.alert('当前标签至少需要 2 个已连接窗格才能开启广播输入。');
      return;
    }
    setBroadcastTargetPaneIds(runtimePaneIds);
    setBroadcastEnabled(true);
    setSessionMessage(`广播输入已开启：当前标签 ${runtimePaneIds.length} 个目标窗格将同步接收输入。`);
  };

  const toggleBroadcastTarget = (paneId: string) => {
    setBroadcastTargetPaneIds((current) => {
      const next = current.includes(paneId) ? current.filter((id) => id !== paneId) : [...current, paneId];
      if (broadcastEnabled && next.length < 2) {
        window.alert('广播输入至少需要保留 2 个目标窗格。');
        return current;
      }
      setSessionMessage(`广播目标已更新：${next.length} 个窗格。`);
      return next;
    });
  };

  const openQuickConnectDialog = (target: { tabId: string; paneId: string } | null = null) => {
    setQuickConnectTarget(target);
    setQuickConnectOpen(true);
  };

  const openLocalInExistingPane = (tabId: string, paneId: string) => {
    setActiveTabId(tabId);
    activatePane(tabId, paneId);
    void openLocalInPane(tabId, paneId, false);
  };

  const disconnectActivePane = () => {
    if (!activePane?.runtimeId) {
      setSessionMessage('当前窗格没有可断开的连接。');
      return;
    }

    const runtimeId = activePane.runtimeId;
    void closeTerminal(runtimeId);
    updatePaneRuntime(activeTab.id, activePane.id, null, `${activePane.title}（已断开）`, 'disconnected', false);
    setBroadcastTargetPaneIds((current) => current.filter((paneId) => paneId !== activePane.id));
    setSessionMessage(`已断开当前窗格：${activePane.title}`);
  };

  const resizeSplit = (tabId: string, splitRatio: number) => {
    setTabs((current) => current.map((tab) => (tab.id === tabId ? { ...tab, splitRatio } : tab)));
  };

  const splitPane = async (tabId: string, direction: SplitDirection) => {
    const paneId = crypto.randomUUID();
    const placeholder = { id: paneId, runtimeId: null, title: '正在打开分屏终端', status: 'connecting' as RuntimeStatus };
    setTabs((current) =>
      current.map((tab) =>
        tab.id === tabId
          ? { ...tab, panes: [...tab.panes, placeholder], activePaneId: paneId, splitDirection: direction, splitRatio: tab.splitRatio ?? 50 }
          : tab,
      ),
    );
    const source = window.prompt('选择新分屏连接来源：local=本地终端，empty=空窗格', 'local')?.trim().toLowerCase() ?? 'local';
    if (source === 'empty') {
      updatePaneRuntime(tabId, paneId, null, '空分屏窗格', 'idle', false);
      setSessionMessage('已创建空分屏窗格，可从会话树或快速连接打开新连接。');
      return;
    }
    try {
      const runtime = await openLocalTerminal(tabId, paneId, 120, 30, settings.terminal);
      updatePaneRuntime(tabId, paneId, runtime.runtimeId, runtime.title, runtime.status, false);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      updatePaneRuntime(tabId, paneId, null, '分屏连接失败', 'failed', false);
      setSessionMessage(`分屏打开失败：${message}`);
    }
  };

  return (
    <div className="app-shell" data-theme={settings.appearance.appTheme}>
      <header className="title-bar xshell-chrome">
        <div className="app-brand">
          <strong>YShell</strong>
          <span>Open SSH Client · Xshell workflow baseline</span>
        </div>
        <nav className="menu-bar" aria-label="Xshell 菜单栏">
          <div className="menu-root"><button type="button">文件(F)</button><div className="menu-panel">
            <button type="button" onClick={() => openQuickConnectDialog()}>新建会话 / 快速连接...</button>
            <button type="button" onClick={() => void openLocal()}>新建本地 Shell</button>
            <button type="button" onClick={disconnectActivePane}>断开当前连接</button>
            <span className="menu-separator" />
            <button type="button" onClick={() => importInputRef.current?.click()}>导入会话...</button>
            <button type="button" onClick={() => void exportSessionFile()}>导出会话...</button>
          </div></div>
          <div className="menu-root"><button type="button">编辑(E)</button><div className="menu-panel">
            <button type="button">复制</button>
            <button type="button">粘贴</button>
            <button type="button">查找...</button>
            <button type="button" disabled>撰写栏 / 撰写窗格</button>
          </div></div>
          <div className="menu-root"><button type="button">查看(V)</button><div className="menu-panel">
            <button type="button">会话管理器</button>
            <button type="button">工具栏</button>
            <button type="button">状态栏</button>
            <button type="button" disabled>全屏</button>
          </div></div>
          <div className="menu-root"><button type="button">选项卡(T)</button><div className="menu-panel">
            <button type="button" onClick={() => openQuickConnectDialog()}>新建 SSH 标签...</button>
            <button type="button" onClick={() => renameTab(activeTab.id)}>重命名当前标签</button>
            <button type="button" onClick={() => toggleTabLock(activeTab.id)}>{activeTab.locked ? '解除锁定当前标签' : '锁定当前标签'}</button>
            <button type="button" onClick={() => closeTab(activeTab.id)}>关闭当前标签</button>
            <button type="button" onClick={() => closeOtherTabs(activeTab.id)}>关闭其他标签</button>
          </div></div>
          <div className="menu-root"><button type="button">窗口(W)</button><div className="menu-panel">
            <button type="button" onClick={() => splitPane(activeTab.id, 'vertical')}>垂直分割窗格</button>
            <button type="button" onClick={() => splitPane(activeTab.id, 'horizontal')}>水平分割窗格</button>
            <button type="button" onClick={() => focusRelativePane(1)}>下一个窗格</button>
            <button type="button" onClick={() => focusRelativePane(-1)}>上一个窗格</button>
            <button type="button" onClick={toggleBroadcast}>{broadcastEnabled ? '停止广播输入' : '广播输入到当前标签'}</button>
          </div></div>
          <div className="menu-root"><button type="button">工具(O)</button><div className="menu-panel">
            <button type="button" disabled>用户密钥管理器</button>
            <button type="button" disabled>主机密钥管理器</button>
            <button type="button" disabled>日志管理器</button>
            <button type="button" onClick={() => setSettingsOpen(true)}>选项...</button>
          </div></div>
          <div className="menu-root"><button type="button">帮助(H)</button><div className="menu-panel"><button type="button">关于 YShell</button></div></div>
        </nav>
        <div className="main-toolbar" aria-label="常用工具栏">
          <button type="button" onClick={() => openQuickConnectDialog()}>快速连接</button>
          <button type="button" onClick={() => splitPane(activeTab.id, 'vertical')}>垂直分屏</button>
          <button type="button" onClick={() => splitPane(activeTab.id, 'horizontal')}>水平分屏</button>
          <button type="button" data-active={broadcastEnabled} onClick={toggleBroadcast}>广播</button>
          <button type="button" onClick={() => setSettingsOpen(true)}>选项</button>
        </div>
        <input ref={importInputRef} className="visually-hidden" type="file" accept="application/json" onChange={(event) => void importSessionFile(event)} />
      </header>
      <main className="main-layout">
        <SessionSidebar
          sessions={sessions}
          onQuickConnect={() => openQuickConnectDialog()}
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
          broadcastEnabled={broadcastEnabled}
          onActivateTab={setActiveTabId}
          onActivatePane={activatePane}
          onOpenQuickConnect={(tabId, paneId) => openQuickConnectDialog({ tabId, paneId })}
          onOpenLocal={openLocalInExistingPane}
          onCloseTab={closeTab}
          onCloseOtherTabs={closeOtherTabs}
          onRenameTab={renameTab}
          onToggleTabLock={toggleTabLock}
          onMoveTab={moveTab}
          onReorderTab={reorderTab}
          onSplitPane={(tabId, direction) => void splitPane(tabId, direction)}
          onResizeSplit={resizeSplit}
          onToggleBroadcast={toggleBroadcast}
          broadcastTargetPaneIds={broadcastTargetPaneIds}
          onToggleBroadcastTarget={toggleBroadcastTarget}
        />
      </main>
      <StatusBar
        pane={activePane}
        loggingEnabled={settings.logging.enabled}
        message={sessionMessage}
        broadcastEnabled={broadcastEnabled}
        broadcastTargetCount={activeBroadcastTargetCount}
      />
      {quickConnectOpen && (
        <QuickConnectPanel
          onCancel={() => {
            setQuickConnectOpen(false);
            setQuickConnectTarget(null);
          }}
          onConnect={openQuickConnection}
        />
      )}
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
      {renamingTabId && (
        <div className="dialog-backdrop" role="presentation">
          <form
            className="dialog compact-dialog"
            onSubmit={(event) => {
              event.preventDefault();
              saveTabRename();
            }}
          >
            <header>
              <span className="eyebrow">Tab</span>
              <h2>重命名标签</h2>
              <p>对应 Xshell 标签上下文菜单中的“重命名”，不打断当前终端会话。</p>
            </header>
            <label>
              标签名称
              <input autoFocus value={renameDraft} onChange={(event) => setRenameDraft(event.target.value)} />
            </label>
            <footer>
              <button type="button" onClick={() => setRenamingTabId(null)}>取消</button>
              <button type="submit" className="primary">保存</button>
            </footer>
          </form>
        </div>
      )}
    </div>
  );
}
