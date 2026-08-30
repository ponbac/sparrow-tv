import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { createSparrowRuntime } from "./client/runtime";
import "@fontsource-variable/archivo";
import "@fontsource-variable/newsreader";
import "@fontsource-variable/jetbrains-mono";
import "./index.css";

const rootElement = requireApplicationRoot();

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});
startApplication().catch(renderStartupFailure);

async function startApplication(): Promise<void> {
  const runtime = await createSparrowRuntime();
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
