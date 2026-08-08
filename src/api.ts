import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface PaneInfo {
  paneId: number;
  cols: number;
  rows: number;
  shell: string;
  ai: boolean;
}

export interface SessionInfo {
  sessionId: number;
  name: string;
  panes: PaneInfo[];
  activePane: number;
}

export interface PaneOutput {
  sessionId: number;
  paneId: number;
  data: string; // base64
}

export interface SpawnRequest {
  name?: string;
  shell?: string;
  cwd?: string;
  cols: number;
  rows: number;
}

export interface SplitRequest {
  sessionId: number;
  shell?: string;
  program?: string;
  args?: string[];
  cwd?: string;
  cols: number;
  rows: number;
  direction: string;
  ai?: boolean;
}

export interface PaneRequest {
  sessionId: number;
  paneId: number;
}

export interface ResizeRequest {
  sessionId: number;
  paneId: number;
  cols: number;
  rows: number;
}

export interface AiPaneRequest {
  sessionId: number;
  cols: number;
  rows: number;
}

export interface EditorContext {
  editor: string;
  file: string | null;
}

export interface GitChange {
  path: string;
  status: string;
  staged: boolean;
}

export interface GitStatus {
  isRepo: boolean;
  branch: string;
  ahead: number;
  behind: number;
  changes: GitChange[];
}

function decodeBase64(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

export async function createSession(req: SpawnRequest): Promise<SessionInfo> {
  return invoke<SessionInfo>("create_session", { request: req });
}

export async function splitPane(req: SplitRequest): Promise<PaneInfo> {
  return invoke<PaneInfo>("split_pane", { request: req });
}

export async function attachPane(req: PaneRequest): Promise<void> {
  await invoke("attach_pane", { request: req });
}

export async function listSessions(): Promise<SessionInfo[]> {
  return invoke<SessionInfo[]>("list_sessions");
}

export async function getSession(sessionId: number): Promise<SessionInfo> {
  return invoke<SessionInfo>("get_session", { sessionId });
}

export async function writePane(req: PaneRequest, data: string): Promise<void> {
  await invoke("write_pane", { request: req, data });
}

export async function resizePane(req: ResizeRequest): Promise<void> {
  await invoke("resize_pane", { request: req });
}

export async function focusPane(req: PaneRequest): Promise<void> {
  await invoke("focus_pane", { request: req });
}

export async function closePane(req: PaneRequest): Promise<boolean> {
  return invoke<boolean>("close_pane", { request: req });
}

export async function closeSession(sessionId: number): Promise<void> {
  await invoke("close_session", { sessionId });
}

export async function defaultShell(): Promise<string> {
  return invoke<string>("default_shell_command");
}

export async function openAiPane(req: AiPaneRequest): Promise<PaneInfo> {
  return invoke<PaneInfo>("open_ai_pane", { request: req });
}

export async function aiCommand(): Promise<string> {
  return invoke<string>("ai_command");
}

export async function aiCommandLine(): Promise<[string, string[]]> {
  return invoke<[string, string[]]>("ai_command_line");
}

export async function editorContext(req: PaneRequest): Promise<EditorContext | null> {
  return invoke<EditorContext | null>("editor_context", { request: req });
}

export async function paneCwd(req: PaneRequest): Promise<string | null> {
  return invoke<string | null>("pane_cwd", { request: req });
}

export async function paneShell(req: PaneRequest): Promise<string> {
  return invoke<string>("pane_shell", { request: req });
}

export async function paneTitle(req: PaneRequest): Promise<string | null> {
  return invoke<string | null>("pane_title", { request: req });
}

export async function gitStatus(): Promise<GitStatus | null> {
  return invoke<GitStatus | null>("git_status");
}

export async function gitDiff(path: string): Promise<string> {
  return invoke<string>("git_diff", { path });
}

export async function saveLayout(layout: string): Promise<void> {
  await invoke("save_layout", { layout });
}

export async function loadLayout(): Promise<string | null> {
  return invoke<string | null>("load_layout");
}

export async function setWorkspace(path: string): Promise<void> {
  await invoke("set_workspace", { path });
}

export async function getWorkspace(): Promise<string | null> {
  return invoke<string | null>("get_workspace");
}

export async function getRecentWorkspaces(): Promise<string[]> {
  return invoke<string[]>("get_recent_workspaces");
}

export function onPaneOutput(cb: (evt: PaneOutput) => void): Promise<UnlistenFn> {
  return listen<PaneOutput>("pane-output", (e) => cb(e.payload));
}

export function onPaneClosed(cb: (evt: { sessionId: number; paneId: number }) => void): Promise<UnlistenFn> {
  return listen("pane-closed", (e) => cb(e.payload as { sessionId: number; paneId: number }));
}

export { decodeBase64 };
