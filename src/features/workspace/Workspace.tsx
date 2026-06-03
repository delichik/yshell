import { useState } from 'react';
import type { SplitDirection, TerminalConfig, WorkspaceTab } from '../../bindings/types';
import { TerminalPane } from '../terminal/TerminalPane';

interface WorkspaceProps {
  tabs: WorkspaceTab[];
  activeTabId: string;
  terminalConfig: TerminalConfig;
  broadcastEnabled: boolean;
  broadcastTargetPaneIds: string[];
  onActivateTab: (tabId: string) => void;
  onActivatePane: (tabId: string, paneId: string) => void;
  onCloseTab: (tabId: string) => void;
  onCloseOtherTabs: (tabId: string) => void;
  onRenameTab: (tabId: string) => void;
  onToggleTabLock: (tabId: string) => void;
  onMoveTab: (tabId: string, direction: -1 | 1) => void;
  onReorderTab: (draggedTabId: string, targetTabId: string) => void;
  onSplitPane: (tabId: string, direction: SplitDirection) => void;
  onResizeSplit: (tabId: string, splitRatio: number) => void;
  onOpenLocalPane: (tabId: string, paneId: string) => void;
  onOpenQuickConnectPane: (tabId: string, paneId: string) => void;
  onToggleBroadcast: () => void;
  onToggleBroadcastTarget: (paneId: string) => void;
}

export function Workspace({
  tabs,
  activeTabId,
  terminalConfig,
  broadcastEnabled,
  broadcastTargetPaneIds,
  onActivateTab,
  onActivatePane,
  onCloseTab,
  onCloseOtherTabs,
  onRenameTab,
  onToggleTabLock,
  onMoveTab,
  onReorderTab,
  onSplitPane,
  onResizeSplit,
  onOpenLocalPane,
  onOpenQuickConnectPane,
  onToggleBroadcast,
  onToggleBroadcastTarget,
}: WorkspaceProps) {
  const [draggingTabId, setDraggingTabId] = useState<string | null>(null);
  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? tabs[0];
  const broadcastTargets = broadcastEnabled
    ? activeTab.panes.filter((pane) => pane.runtimeId && broadcastTargetPaneIds.includes(pane.id)).map((pane) => pane.id)
    : [];
  const broadcastRuntimeIds = activeTab.panes
    .filter((pane) => pane.runtimeId && broadcastTargets.includes(pane.id))
    .map((pane) => pane.runtimeId)
    .filter((runtimeId): runtimeId is string => Boolean(runtimeId));
  const splitRatio = activeTab.splitRatio ?? 50;
  const paneGridStyle =
    activeTab.panes.length === 2
      ? activeTab.splitDirection === 'horizontal'
        ? { gridTemplateRows: `${splitRatio}% minmax(0, 1fr)` }
        : { gridTemplateColumns: `${splitRatio}% minmax(0, 1fr)` }
      : undefined;

  return (
    <section className="workspace" aria-label="终端工作区" data-broadcast={broadcastEnabled}>
      <div className="workspace-commandbar" aria-label="Xshell 工作台工具栏">
        <div className="command-group" aria-label="连接">
          <button type="button" onClick={() => onOpenQuickConnectPane(activeTab.id, activeTab.activePaneId)}>快速连接</button>
          <button type="button" onClick={() => onOpenLocalPane(activeTab.id, activeTab.activePaneId)}>本地 Shell</button>
        </div>
        <div className="command-group" aria-label="分屏">
          <button type="button" onClick={() => onSplitPane(activeTab.id, 'vertical')}>垂直分割</button>
          <button type="button" onClick={() => onSplitPane(activeTab.id, 'horizontal')}>水平分割</button>
          {activeTab.panes.length > 1 && (
            <label className="split-slider" title="拖动分隔条调整分屏比例">
              分隔条
              <input
                aria-label="拖动分隔条调整分屏比例"
                max="75"
                min="25"
                type="range"
                value={splitRatio}
                onChange={(event) => onResizeSplit(activeTab.id, Number(event.target.value))}
              />
            </label>
          )}
        </div>
        <div className="command-group" aria-label="标签">
          <button type="button" onClick={() => onRenameTab(activeTab.id)}>重命名</button>
          <button type="button" onClick={() => onToggleTabLock(activeTab.id)}>{activeTab.locked ? '解除锁定' : '锁定'}</button>
          <button type="button" onClick={() => onCloseOtherTabs(activeTab.id)}>关闭其他</button>
        </div>
        <div className="command-group" aria-label="广播输入">
          <button className="broadcast-toggle" data-active={broadcastEnabled} type="button" onClick={onToggleBroadcast}>
            {broadcastEnabled ? `停止广播 (${broadcastTargets.length})` : '广播输入'}
          </button>
        </div>
      </div>

      <div className="tab-strip" role="tablist" aria-label="会话标签">
        {tabs.map((tab, index) => {
          const connected = tab.panes.some((pane) => pane.status === 'connected' || pane.status === 'connecting');
          return (
            <button
              className="tab"
              data-active={tab.id === activeTabId}
              data-dragging={draggingTabId === tab.id}
              data-locked={tab.locked}
              draggable
              key={tab.id}
              role="tab"
              type="button"
              onClick={() => onActivateTab(tab.id)}
              onDoubleClick={() => onRenameTab(tab.id)}
              onDragStart={(event) => {
                setDraggingTabId(tab.id);
                event.dataTransfer.effectAllowed = 'move';
                event.dataTransfer.setData('text/plain', tab.id);
              }}
              onDragOver={(event) => {
                event.preventDefault();
                event.dataTransfer.dropEffect = 'move';
              }}
              onDrop={(event) => {
                event.preventDefault();
                const draggedId = event.dataTransfer.getData('text/plain') || draggingTabId;
                if (draggedId) onReorderTab(draggedId, tab.id);
                setDraggingTabId(null);
              }}
              onDragEnd={() => setDraggingTabId(null)}
              title="拖拽排序，双击重命名标签"
            >
              <span className="tab-title">{tab.locked ? '🔒 ' : ''}{tab.title}</span>
              <span className="tab-status" aria-label={connected ? '有活跃连接' : '无活跃连接'}>{connected ? '●' : '○'}</span>
              <span className="tab-mini-actions" aria-label="标签排序">
                <span
                  className="tab-action"
                  role="button"
                  aria-disabled={index === 0}
                  tabIndex={0}
                  onClick={(event) => {
                    event.stopPropagation();
                    onMoveTab(tab.id, -1);
                  }}
                >
                  ◀
                </span>
                <span
                  className="tab-action"
                  role="button"
                  aria-disabled={index === tabs.length - 1}
                  tabIndex={0}
                  onClick={(event) => {
                    event.stopPropagation();
                    onMoveTab(tab.id, 1);
                  }}
                >
                  ▶
                </span>
              </span>
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
          );
        })}
      </div>

      {broadcastEnabled && (
        <div className="broadcast-targets" aria-label="广播目标选择">
          <strong>广播目标</strong>
          {activeTab.panes.map((pane) => (
            <label key={pane.id}>
              <input
                checked={broadcastTargetPaneIds.includes(pane.id)}
                disabled={!pane.runtimeId}
                type="checkbox"
                onChange={() => onToggleBroadcastTarget(pane.id)}
              />
              {pane.title}{pane.runtimeId ? '' : '（未连接）'}
            </label>
          ))}
        </div>
      )}

      <div className="pane-grid" data-layout={activeTab.splitDirection ?? 'vertical'} style={paneGridStyle}>
        {activeTab.panes.map((pane) => (
          <TerminalPane
            config={terminalConfig}
            key={pane.id}
            pane={pane}
            active={pane.id === activeTab.activePaneId}
            broadcastEnabled={broadcastEnabled}
            broadcastTargetCount={broadcastTargets.length}
            broadcastTargetRuntimeIds={broadcastRuntimeIds}
            markedForBroadcast={broadcastTargets.includes(pane.id)}
            onActivate={() => onActivatePane(activeTab.id, pane.id)}
            onOpenLocal={() => onOpenLocalPane(activeTab.id, pane.id)}
            onOpenQuickConnect={() => onOpenQuickConnectPane(activeTab.id, pane.id)}
          />
        ))}
      </div>
    </section>
  );
}
