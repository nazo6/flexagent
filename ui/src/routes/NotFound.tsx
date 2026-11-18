import ErrorPage from "./ErrorPage.tsx";

/** Unknown route (`*404`), rendered inside the app shell. */
export default function NotFound() {
  return <ErrorPage notFound />;
}
