import { render } from "@solidjs/web";
import "./styles.css";
import { Router } from "./app/router";

render(() => <Router />, document.getElementById("root")!);
