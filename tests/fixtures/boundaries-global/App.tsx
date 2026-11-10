import { lazy, Suspense } from "react";
import { ErrorBoundary } from "./ErrorBoundary";
import { Outlet, Routes } from "./router";

const Lazy = lazy(() => import("./Lazy"));
const Other = lazy(() => import("./Other"));

function PageX() {
  return <p />;
}

function Layout() {
  return (
    <Suspense fallback={null}>
      <Outlet />
    </Suspense>
  );
}

// the router is inside a boundary: pages written in the component are covered, even though the
// config is built outside the JSX
export function App() {
  const routes = [
    { path: "/x", element: <PageX /> },
    { path: "/l", element: <Lazy /> },
  ];
  return (
    <ErrorBoundary>
      <Routes routes={routes} />
      <Layout />
      <Other />
    </ErrorBoundary>
  );
}
