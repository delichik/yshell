import { invoke } from '@tauri-apps/api/core';
import type { AppSettings, QuickConnectDraft, SessionProfile, TerminalRuntime } from './types';

export const runningInTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export async function listSessions(): Promise<SessionProfile[]> {
  if (!runningInTauri) return [];
  return invoke<SessionProfile[]>('sessions_list');
}

export async function saveSession(profile: SessionProfile): Promise<SessionProfile> {
  if (!runningInTauri) return profile;
  return invoke<SessionProfile>('sessions_save', { profile });
}

export async function openLocalTerminal(tabId: string, paneId: string, cols = 120, rows = 30): Promise<TerminalRuntime> {
  if (!runningInTauri) {
    return {
      runtimeId: `preview-${paneId}`,
      profileId: null,
      kind: 'local',
      status: 'connected',
      paneId,
      tabId,
      title: 'Preview Shell',
    };
  }
  return invoke<TerminalRuntime>('terminal_open_local', { tabId, paneId, cols, rows });
}

export async function openSshTerminal(draft: QuickConnectDraft, tabId: string, paneId: string): Promise<TerminalRuntime> {
  if (!runningInTauri) {
    return {
      runtimeId: `preview-${paneId}`,
      profileId: null,
      kind: 'ssh',
      status: 'connecting',
      paneId,
      tabId,
      title: draft.name || `${draft.username}@${draft.host}`,
    };
  }
  return invoke<TerminalRuntime>('terminal_open_ssh', { draft, tabId, paneId });
}

export async function writeTerminal(runtimeId: string, data: string): Promise<void> {
  if (!runningInTauri) return;
  return invoke('terminal_write', { runtimeId, data });
}

export async function resizeTerminal(runtimeId: string, cols: number, rows: number): Promise<void> {
  if (!runningInTauri) return;
  return invoke('terminal_resize', { runtimeId, cols, rows });
}

export async function closeTerminal(runtimeId: string): Promise<void> {
  if (!runningInTauri) return;
  return invoke('terminal_close', { runtimeId });
}

export async function loadSettings(): Promise<AppSettings | null> {
  if (!runningInTauri) return null;
  return invoke<AppSettings>('settings_load');
}
