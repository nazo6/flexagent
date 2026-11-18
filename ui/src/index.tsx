import { render } from "@solidjs/web";
import { Errored } from "solid-js";
import "./styles.css";
import { Router } from "./app/router";
import ErrorPage from "./routes/ErrorPage.tsx";

render(
  () => (
    <Errored fallback={(error) => <ErrorPage class="min-h-svh" error={error()} />}>
      <Router />
    </Errored>
  ),
  document.getElementById("root")!,
);

if (import.meta.env.PROD && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    void navigator.serviceWorker.register("/sw.js");
  });
}
