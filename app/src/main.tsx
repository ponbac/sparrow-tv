import { QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { createSparrowRuntime } from "./client/runtime";
import { createSparrowQueryClient } from "./client/query-client";
import "@fontsource-variable/saira/wdth.css";
import "./index.css";

const rootElement = requireApplicationRoot();

startApplication().catch(renderStartupFailure);

async function startApplication(): Promise<void> {
  const runtime = await createSparrowRuntime();
  const queryClient = createSparrowQueryClient(runtime._tag);
  createRoot(rootElement).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <App runtime={runtime} />
      </QueryClientProvider>
    </StrictMode>,
  );
}

function renderStartupFailure(): void {
  rootElement.replaceChildren();
  const message = document.createElement("p");
  message.className = "startup-failure";
  message.setAttribute("role", "alert");
  message.textContent = "Sparrow could not start. Close the app and try again.";
  rootElement.append(message);
}

function requireApplicationRoot(): HTMLElement {
  const element = document.getElementById("root");
  if (element === null) {
    throw new Error("Sparrow application root is missing");
  }
  return element;
}
