import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import type { TerminalConfig, WorkspacePane } from '../../bindings/types';

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
    terminal.writeln('YShell engineering baseline');
    terminal.writeln(`runtime: ${pane.runtimeId ?? 'not connected'} | status: ${pane.status}`);
    terminal.writeln('PTY/SSH event streaming is isolated behind the Tauri terminal runtime registry.');
    const observer = new ResizeObserver(() => fitAddon.fit());
    observer.observe(hostRef.current);
    return () => {
      observer.disconnect();
      terminal.dispose();
    };
  }, [config, pane.runtimeId, pane.status]);

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
