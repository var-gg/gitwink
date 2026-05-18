import { invoke } from "@tauri-apps/api/core";

export async function ping(): Promise<string> {
  return invoke<string>("ping");
}
