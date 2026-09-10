import { useEffect } from "react";
import { installAgentControlDispatch } from "./agent-control-binding";
import type { AgentControlResponse } from "./agent-control";

/**
 * Installs the window dispatch Rust evals for Agent Control. Renders nothing.
 * Loaded only in the installed composition so hosted bundles never import Tauri.
 */
export function InstalledAgentBridge() {
  useEffect(() => {
    return installAgentControlDispatch(replyAgentControl);
  }, []);
  return null;
}

async function replyAgentControl(
  id: string,
  body: AgentControlResponse,
): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("agent_control_reply", { id, body });
}
