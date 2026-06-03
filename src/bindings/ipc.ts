import { invoke } from '@tauri-apps/api/core';
import type { AppSettings, QuickConnectDraft, SessionExportBundle, SessionProfile, TerminalConfig, TerminalRuntime } from './types';

export const runningInTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const previewSessionsKey = 'yshell.preview.sessions';
const previewSettingsKey = 'yshell.preview.settings';

export async function listSessions(): Promise<SessionProfile[]> {
  if (!runningInTauri) return readPreviewJson<SessionProfile[]>(previewSessionsKey, []);
  return invoke<SessionProfile[]>('sessions_list');
}

export async function saveSession(profile: SessionProfile): Promise<SessionProfile> {
  if (!runningInTauri) {
    const sessions = readPreviewJson<SessionProfile[]>(previewSessionsKey, []);
    writePreviewJson(previewSessionsKey, [sanitizeSession(profile), ...sessions.filter((session) => session.id !== profile.id)]);
    return sanitizeSession(profile);
  }
  return invoke<SessionProfile>('sessions_save', { profile });
}

export async function deleteSession(sessionId: string): Promise<SessionProfile[]> {
  if (!runningInTauri) {
    const sessions = readPreviewJson<SessionProfile[]>(previewSessionsKey, []).filter((session) => session.id !== sessionId);
    writePreviewJson(previewSessionsKey, sessions);
    return sessions;
  }
  return invoke<SessionProfile[]>('sessions_delete', { sessionId });
}

export async function duplicateSession(sessionId: string): Promise<SessionProfile> {
  if (!runningInTauri) {
    const sessions = readPreviewJson<SessionProfile[]>(previewSessionsKey, []);
    const source = sessions.find((session) => session.id === sessionId);
    if (!source) throw new Error(`session ${sessionId} not found`);
    const now = new Date().toISOString();
    const clone = sanitizeSession({
      ...source,
      id: crypto.randomUUID(),
      name: `${source.name} 副本`,
      createdAt: now,
      updatedAt: now,
      lastConnectedAt: null,
    });
    writePreviewJson(previewSessionsKey, [clone, ...sessions]);
    return clone;
  }
  return invoke<SessionProfile>('sessions_duplicate', { sessionId });
}

export async function exportSessions(): Promise<SessionExportBundle> {
  if (!runningInTauri) {
    return {
      version: 1,
      exportedAt: new Date().toISOString(),
      sessions: readPreviewJson<SessionProfile[]>(previewSessionsKey, []).map(sanitizeSessionForExport),
    };
  }
  return invoke<SessionExportBundle>('sessions_export');
}

export async function importSessions(bundle: SessionExportBundle): Promise<SessionProfile[]> {
  if (!runningInTauri) {
    const current = readPreviewJson<SessionProfile[]>(previewSessionsKey, []);
    const imported = bundle.sessions.map(sanitizeSessionForImport);
    const merged = [...imported, ...current.filter((session) => !imported.some((item) => item.id === session.id))];
    writePreviewJson(previewSessionsKey, merged);
    return merged;
  }
  return invoke<SessionProfile[]>('sessions_import', { bundle });
}

export async function openLocalTerminal(
  tabId: string,
  paneId: string,
  cols = 120,
  rows = 30,
  terminal?: TerminalConfig,
): Promise<TerminalRuntime> {
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
  return invoke<TerminalRuntime>('terminal_open_local', { tabId, paneId, cols, rows, terminal });
}

export async function openSshTerminal(draft: QuickConnectDraft, tabId: string, paneId: string): Promise<TerminalRuntime> {
  if (!runningInTauri) {
    return {
      runtimeId: `preview-${paneId}`,
      profileId: null,
      kind: 'ssh',
      status: 'failed',
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

export async function closeAllTerminals(): Promise<void> {
  if (!runningInTauri) return;
  return invoke('terminal_close_all');
}

export async function loadSettings(): Promise<AppSettings | null> {
  if (!runningInTauri) return readPreviewJson<AppSettings | null>(previewSettingsKey, null);
  return invoke<AppSettings>('settings_load');
}

export async function saveSettings(settings: AppSettings): Promise<AppSettings> {
  if (!runningInTauri) {
    writePreviewJson(previewSettingsKey, settings);
    return settings;
  }
  return invoke<AppSettings>('settings_save', { settings });
}

function sanitizeSession(profile: SessionProfile): SessionProfile {
  return {
    ...profile,
    auth: {
      method: profile.auth.method,
      username: profile.auth.username,
      privateKeyPath: profile.auth.privateKeyPath,
      credentialRef: profile.auth.credentialRef,
    },
  };
}

function sanitizeSessionForExport(profile: SessionProfile): SessionProfile {
  const session = sanitizeSession(profile);
  return { ...session, auth: { ...session.auth, credentialRef: undefined } };
}

function sanitizeSessionForImport(profile: SessionProfile): SessionProfile {
  const session = sanitizeSession(profile);
  return { ...session, auth: { ...session.auth, credentialRef: undefined } };
}

function readPreviewJson<T>(key: string, fallback: T): T {
  if (typeof window === 'undefined') return fallback;
  const rawValue = window.localStorage.getItem(key);
  if (!rawValue) return fallback;
  try {
    return JSON.parse(rawValue) as T;
  } catch {
    return fallback;
  }
}

function writePreviewJson(key: string, value: unknown) {
  if (typeof window === 'undefined') return;
  window.localStorage.setItem(key, JSON.stringify(value));
}
