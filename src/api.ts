import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface PaneInfo {
  paneId: number;
  cols: number;
  rows: number;
  shell: string;
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
  cols: number;
  rows: number;
}

export interface SplitRequest {
  sessionId: number;
  shell?: string;
  cols: number;
  rows: number;
  direction: string;
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

export function onPaneOutput(cb: (evt: PaneOutput) => void): Promise<UnlistenFn> {
  return listen<PaneOutput>("pane-output", (e) => cb(e.payload));
}

export function onPaneClosed(cb: (evt: { sessionId: number; paneId: number }) => void): Promise<UnlistenFn> {
  return listen("pane-closed", (e) => cb(e.payload as { sessionId: number; paneId: number }));
}

export { decodeBase64 };
