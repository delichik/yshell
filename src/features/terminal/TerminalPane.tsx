import { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { SearchAddon } from '@xterm/addon-search';
import '@xterm/xterm/css/xterm.css';
import { resizeTerminal, runningInTauri, writeTerminal } from '../../bindings/ipc';
import type { TerminalConfig, TerminalOutputEvent, WorkspacePane } from '../../bindings/types';

const dangerousCommandPatterns = ['rm -rf', 'mkfs', 'reboot', 'shutdown', 'poweroff', 'dd if=', ':(){ :|:& };:'];

interface TerminalPaneProps {
  pane: WorkspacePane;
  config: TerminalConfig;
  active: boolean;
  broadcastEnabled: boolean;
  broadcastTargetCount: number;
  broadcastTargetRuntimeIds: string[];
  markedForBroadcast: boolean;
  onActivate: () => void;
  onOpenLocal: () => void;
  onOpenQuickConnect: () => void;
}

export function TerminalPane({
  pane,
  config,
  active,
  broadcastEnabled,
  broadcastTargetCount,
  broadcastTargetRuntimeIds,
  markedForBroadcast,
  onActivate,
  onOpenLocal,
  onOpenQuickConnect,
}: TerminalPaneProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const searchRef = useRef<SearchAddon | null>(null);
  const resizeTimerRef = useRef<number | null>(null);
  const commandBufferRef = useRef('');
  const activeRef = useRef(active);
  const broadcastEnabledRef = useRef(broadcastEnabled);
  const broadcastTargetRuntimeIdsRef = useRef(broadcastTargetRuntimeIds);
  const [searchQuery, setSearchQuery] = useState('');

  activeRef.current = active;
  broadcastEnabledRef.current = broadcastEnabled;
  broadcastTargetRuntimeIdsRef.current = broadcastTargetRuntimeIds;

  useEffect(() => {
    if (!active) return;
    terminalRef.current?.focus();
  }, [active]);

  useEffect(() => {
    if (!hostRef.current || !pane.runtimeId) return undefined;
    const fitAddon = new FitAddon();
    const searchAddon = new SearchAddon();
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: config.fontFamily,
      fontSize: config.fontSize,
      lineHeight: config.lineHeight,
      scrollback: config.scrollback,
      theme: terminalTheme(config.colorScheme),
    });

    terminalRef.current = terminal;
    searchRef.current = searchAddon;
    terminal.loadAddon(fitAddon);
    terminal.loadAddon(searchAddon);
    terminal.open(hostRef.current);
    fitAddon.fit();

    if (!runningInTauri) {
      terminal.writeln('YShell browser preview');
      terminal.writeln('输入内容会在预览模式中本地回显；Tauri 模式会写入后端 shell stdin。');
    }

    const inputDisposable = terminal.onData((data: string) => {
      if (!activeRef.current) return;
      void sendInput(data, looksLikePaste(data));
    });

    let unlisten: UnlistenFn | undefined;
    if (pane.runtimeId && runningInTauri) {
      void listen<TerminalOutputEvent>('terminal://output', (event) => {
        if (event.payload.runtimeId === pane.runtimeId) {
          terminal.write(event.payload.data);
        }
      }).then((cleanup) => {
        unlisten = cleanup;
      });
    }

    const resize = () => {
      fitAddon.fit();
      if (!pane.runtimeId) return;
      if (resizeTimerRef.current !== null) {
        window.clearTimeout(resizeTimerRef.current);
      }
      resizeTimerRef.current = window.setTimeout(() => {
        resizeTimerRef.current = null;
        void resizeTerminal(pane.runtimeId as string, terminal.cols, terminal.rows);
      }, 80);
    };
    const observer = new ResizeObserver(resize);
    observer.observe(hostRef.current);
    resize();

    return () => {
      unlisten?.();
      inputDisposable.dispose();
      observer.disconnect();
      if (resizeTimerRef.current !== null) {
        window.clearTimeout(resizeTimerRef.current);
        resizeTimerRef.current = null;
      }
      terminal.dispose();
      terminalRef.current = null;
      searchRef.current = null;
    };
  }, [config.colorScheme, config.fontFamily, config.fontSize, config.lineHeight, config.scrollback, pane.runtimeId]);

  const sendInput = async (data: string, fromPaste: boolean) => {
    const runtimeIds = broadcastEnabledRef.current ? broadcastTargetRuntimeIdsRef.current : pane.runtimeId ? [pane.runtimeId] : [];
    if (runtimeIds.length === 0) return;

    if (requiresInputConfirmation(data, fromPaste) && !confirmRiskyInput(data, runtimeIds.length)) {
      terminalRef.current?.writeln('\r\n已取消发送。');
      return;
    }

    if (!fromPaste && broadcastEnabledRef.current && data.includes('\r')) {
      const command = commandBufferRef.current;
      commandBufferRef.current = '';
      if (containsDangerousCommand(command) && !window.confirm(`检测到危险命令：${command}\n\n广播目标：${runtimeIds.length} 个窗格。确认发送回车执行吗？`)) {
        terminalRef.current?.writeln('\r\n危险命令已拦截，未发送回车。');
        return;
      }
    } else if (!fromPaste) {
      if (data.includes('\u007f')) {
        commandBufferRef.current = commandBufferRef.current.slice(0, -1);
      }
      commandBufferRef.current += data.replace(/[\u0000-\u001f\u007f]/g, '');
    }

    if (!runningInTauri) {
      terminalRef.current?.write(data.replace(/\r/g, '\r\n'));
      return;
    }

    await Promise.all(runtimeIds.map((runtimeId) => writeTerminal(runtimeId, data)));
  };

  const copySelection = async () => {
    const selection = terminalRef.current?.getSelection();
    if (!selection) return;
    await navigator.clipboard.writeText(selection);
  };

  const pasteClipboard = async () => {
    const content = await navigator.clipboard.readText();
    if (!content) return;
    await sendInput(content, true);
  };

  const findNext = () => {
    if (!searchQuery) return;
    searchRef.current?.findNext(searchQuery);
  };

  if (!pane.runtimeId) {
    return (
      <article
        className="terminal-pane terminal-pane-empty"
        data-active={active}
        data-broadcast-target={false}
        onMouseDown={onActivate}
      >
        <header>
          <strong>{pane.title}</strong>
          <div className="terminal-actions"><span>{pane.status}</span></div>
        </header>
        <div className="terminal-source-picker">
          <span className="eyebrow">Connection Source</span>
          <h3>选择连接来源</h3>
          <p>像 Xshell 分屏一样，先创建窗格，再在窗格中打开 SSH 快速连接或本地 Shell；不会弹出临时终端窗口。</p>
          <div className="source-actions">
            <button type="button" onClick={onOpenQuickConnect}>SSH 快速连接</button>
            <button type="button" onClick={onOpenLocal}>本地 Shell</button>
          </div>
        </div>
      </article>
    );
  }

  return (
    <article
      className="terminal-pane"
      data-active={active}
      data-broadcast-target={markedForBroadcast}
      onMouseDown={onActivate}
    >
      <header>
        <strong>{pane.title}</strong>
        <div className="terminal-actions">
          {markedForBroadcast && <span className="broadcast-pill">广播中</span>}
          <input
            aria-label="搜索当前终端"
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') findNext();
            }}
            placeholder="搜索…"
          />
          <button type="button" onClick={findNext}>查找</button>
          <button type="button" onClick={() => void copySelection()}>复制</button>
          <button type="button" onClick={() => void pasteClipboard()}>粘贴</button>
          <span>{pane.status}</span>
          {broadcastEnabled && <span className="target-count">目标 {broadcastTargetCount}</span>}
        </div>
      </header>
      <div className="terminal-host" ref={hostRef} onFocus={onActivate} />
    </article>
  );
}

function requiresInputConfirmation(data: string, fromPaste: boolean) {
  return (fromPaste && lineCount(data) > 1) || containsDangerousCommand(data);
}

function looksLikePaste(data: string) {
  return lineCount(data) > 1 || data.length > 128;
}

function lineCount(data: string) {
  return data.split(/\r\n|\r|\n/).filter((line) => line.trim().length > 0).length;
}

function confirmRiskyInput(data: string, targetCount: number) {
  const lines = data.split(/\r?\n/).filter(Boolean);
  const preview = lines.slice(0, 3).join('\n') || data.slice(0, 120);
  const risky = containsDangerousCommand(data) ? '\n\n检测到危险命令关键字。' : '';
  return window.confirm(`确认发送输入到 ${targetCount} 个目标窗格吗？\n\n预览：\n${preview}${risky}`);
}

function containsDangerousCommand(data: string) {
  const normalized = data.toLowerCase();
  return dangerousCommandPatterns.some((pattern) => normalized.includes(pattern.toLowerCase()));
}

function terminalTheme(colorScheme: string) {
  if (colorScheme.toLowerCase().includes('light')) {
    return {
      background: '#fbfbfb',
      foreground: '#1f2937',
      cursor: '#2563eb',
      selectionBackground: '#bfdbfe',
    };
  }
  return {
    background: '#0f1117',
    foreground: '#d7dae0',
    cursor: '#79c0ff',
    selectionBackground: '#264f78',
  };
}
