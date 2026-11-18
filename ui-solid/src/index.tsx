import { render } from "@solidjs/web";
import "./styles.css";
import { Router } from "./app/router";

render(() => <Router />, document.getElementById("root")!);

if (import.meta.env.PROD && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    void navigator.serviceWorker.register("/sw.js");
  });
}
