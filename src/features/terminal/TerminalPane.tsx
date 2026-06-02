import { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { SearchAddon } from '@xterm/addon-search';
import '@xterm/xterm/css/xterm.css';
import { resizeTerminal, runningInTauri, writeTerminal } from '../../bindings/ipc';
import type { TerminalConfig, TerminalOutputEvent, WorkspacePane } from '../../bindings/types';

interface TerminalPaneProps {
  pane: WorkspacePane;
  config: TerminalConfig;
  active: boolean;
}

export function TerminalPane({ pane, config, active }: TerminalPaneProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const searchRef = useRef<SearchAddon | null>(null);
  const [searchQuery, setSearchQuery] = useState('');

  useEffect(() => {
    if (!hostRef.current) return undefined;
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

    if (!pane.runtimeId) {
      terminal.writeln('YShell local terminal is ready to open a runtime.');
    } else if (!runningInTauri) {
      terminal.writeln('YShell browser preview');
      terminal.writeln('输入内容会在预览模式中本地回显；Tauri 模式会写入后端 shell stdin。');
    }

    const inputDisposable = terminal.onData((data: string) => {
      if (!pane.runtimeId) return;
      if (!runningInTauri) {
        terminal.write(data.replace(/\r/g, '\r\n'));
        return;
      }
      void writeTerminal(pane.runtimeId, data);
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
      if (pane.runtimeId) {
        void resizeTerminal(pane.runtimeId, terminal.cols, terminal.rows);
      }
    };
    const observer = new ResizeObserver(resize);
    observer.observe(hostRef.current);
    resize();

    return () => {
      unlisten?.();
      inputDisposable.dispose();
      observer.disconnect();
      terminal.dispose();
      terminalRef.current = null;
      searchRef.current = null;
    };
  }, [config.colorScheme, config.fontFamily, config.fontSize, config.lineHeight, config.scrollback, pane.runtimeId]);

  const copySelection = async () => {
    const selection = terminalRef.current?.getSelection();
    if (!selection) return;
    await navigator.clipboard.writeText(selection);
  };

  const pasteClipboard = async () => {
    if (!pane.runtimeId) return;
    const content = await navigator.clipboard.readText();
    if (!content) return;
    if (!runningInTauri) {
      terminalRef.current?.write(content.replace(/\r?\n/g, '\r\n'));
      return;
    }
    await writeTerminal(pane.runtimeId, content);
  };

  const findNext = () => {
    if (!searchQuery) return;
    searchRef.current?.findNext(searchQuery);
  };

  return (
    <article className="terminal-pane" data-active={active}>
      <header>
        <strong>{pane.title}</strong>
        <div className="terminal-actions">
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
        </div>
      </header>
      <div className="terminal-host" ref={hostRef} />
    </article>
  );
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
