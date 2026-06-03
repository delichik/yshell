export type SessionProtocol = 'local' | 'ssh';
export type RuntimeStatus = 'idle' | 'connecting' | 'connected' | 'disconnected' | 'failed';
export type AuthMethod = 'password' | 'private_key' | 'agent';
export type AppTheme = 'system' | 'light' | 'dark';

export interface AuthConfig {
  method: AuthMethod;
  username?: string;
  privateKeyPath?: string;
  credentialRef?: string;
}

export type HostKeyPolicy = 'strict' | 'accept_new' | 'prompt';

export interface HostKeyState {
  status: 'trusted' | 'unknown' | 'changed';
  fingerprint?: string;
  message: string;
}

export interface TerminalConfig {
  shell?: string;
  workingDirectory?: string;
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  colorScheme: string;
  scrollback: number;
}

export interface LoggingConfig {
  enabled: boolean;
  directory?: string;
  namingTemplate: string;
  redactSensitiveInput: boolean;
}

export interface AppearanceConfig {
  appTheme: AppTheme;
  terminalOpacity: number;
  reduceMotion: boolean;
}

export interface ProxyConfig {
  kind: 'http' | 'socks5' | 'jump_host';
  host: string;
  port: number;
  username?: string;
}

export interface SessionProfile {
  id: string;
  name: string;
  folderId: string | null;
  tags: string[];
  protocol: SessionProtocol;
  host: string | null;
  port: number | null;
  username: string | null;
  auth: AuthConfig;
  proxy: ProxyConfig | null;
  terminal: TerminalConfig;
  appearance: AppearanceConfig | null;
  logging: LoggingConfig | null;
  createdAt: string;
  updatedAt: string;
  lastConnectedAt: string | null;
}

export interface TerminalRuntime {
  runtimeId: string;
  profileId: string | null;
  kind: SessionProtocol;
  status: RuntimeStatus;
  paneId: string;
  tabId: string;
  title: string;
}

export interface TerminalOutputEvent {
  runtimeId: string;
  data: string;
}

export interface TerminalStatusEvent {
  runtimeId: string;
  status: RuntimeStatus;
}

export interface WorkspacePane {
  id: string;
  runtimeId: string | null;
  title: string;
  status: RuntimeStatus;
}

export interface WorkspaceTab {
  id: string;
  title: string;
  locked: boolean;
  panes: WorkspacePane[];
  activePaneId: string;
}

export interface QuickConnectDraft {
  protocol: SessionProtocol;
  name: string;
  host: string;
  port: number;
  username: string;
  authMethod: AuthMethod;
  privateKeyPath?: string;
  hostKeyPolicy: HostKeyPolicy;
  saveAsSession: boolean;
}

export interface SessionExportBundle {
  version: number;
  exportedAt: string;
  sessions: SessionProfile[];
}

export interface AppSettings {
  appearance: AppearanceConfig;
  terminal: TerminalConfig;
  logging: LoggingConfig;
}

export const defaultTerminalConfig: TerminalConfig = {
  fontFamily: 'Cascadia Mono, JetBrains Mono, SFMono-Regular, Consolas, monospace',
  fontSize: 14,
  lineHeight: 1.25,
  colorScheme: 'One Dark',
  scrollback: 10_000,
};

export const defaultAppearance: AppearanceConfig = {
  appTheme: 'dark',
  terminalOpacity: 1,
  reduceMotion: false,
};

export const defaultLogging: LoggingConfig = {
  enabled: false,
  namingTemplate: '{session}-{host}-{date}.log',
  redactSensitiveInput: true,
};
