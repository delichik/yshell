import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
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

  useEffect(() => {
    if (!hostRef.current) return undefined;
    const fitAddon = new FitAddon();
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: config.fontFamily,
      fontSize: config.fontSize,
      lineHeight: config.lineHeight,
      scrollback: config.scrollback,
      theme: {
        background: '#0f1117',
        foreground: '#d7dae0',
        cursor: '#79c0ff',
        selectionBackground: '#264f78',
      },
    });

    terminal.loadAddon(fitAddon);
    terminal.open(hostRef.current);
    fitAddon.fit();

    if (!pane.runtimeId) {
      terminal.writeln('YShell local terminal is ready to open a runtime.');
    } else if (!runningInTauri) {
      terminal.writeln('YShell browser preview');
      terminal.writeln('输入内容会在预览模式中本地回显；Tauri 模式会写入后端 shell stdin。');
    }

    const inputDisposable = terminal.onData((data) => {
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
    };
  }, [config.colorScheme, config.fontFamily, config.fontSize, config.lineHeight, config.scrollback, pane.runtimeId]);

  return (
    <article className="terminal-pane" data-active={active}>
      <header>
        <strong>{pane.title}</strong>
        <span>{pane.status}</span>
      </header>
      <div className="terminal-host" ref={hostRef} />
    </article>
  );
}
