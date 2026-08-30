import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { createHttpSparrowClient } from "./client/http";
import "@fontsource-variable/archivo";
import "@fontsource-variable/newsreader";
import "@fontsource-variable/jetbrains-mono";
import "./index.css";

const rootElement = document.getElementById("root");
if (rootElement === null) {
  throw new Error("Sparrow application root is missing");
}

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});
const client = createHttpSparrowClient();

createRoot(rootElement).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <App client={client} />
    </QueryClientProvider>
  </StrictMode>,
);
