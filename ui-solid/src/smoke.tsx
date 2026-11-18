import { render } from "@solidjs/web";
import { createRouter } from "@solidjs/router";
import SessionPage from "./routes/SessionPage";
import NewSessionChat from "./lib/components/session/NewSessionChat";
import ChatTimeline from "./lib/components/chat/ChatTimeline";
import type { TimelineItem } from "./lib/sync/reducer";

history.replaceState(null, "", "/sessions/smoke-session");

const items: TimelineItem[] = [
  {
    kind: "user",
    key: "u1",
    seq: 1,
    createdAt: Date.now(),
    text: "hello **world**",
    attachments: [],
    clientSource: "web",
  },
  {
    kind: "agent",
    key: "a1",
    seq: 2,
    createdAt: Date.now(),
    text: "# Title\n\n```ts\nconst x: number = 1;\n```",
    streaming: false,
    subagentId: null,
  },
];

const routes = [
  {
    path: "/",
    component: () => (
      <div>
        <a href="/new">go new</a>
        <a href="/timeline">go timeline</a>
      </div>
    ),
  },
  { path: "/sessions/:id", component: SessionPage },
  { path: "/new", component: () => <NewSessionChat /> },
  {
    path: "/timeline",
    component: () => <ChatTimeline sessionId="smoke" items={items} status={null} />,
  },
];
const Router = createRouter({ routes });

render(() => <Router />, document.getElementById("app")!);
