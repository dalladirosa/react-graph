import { lazy } from "react";

const Lazy = lazy(() => import("./LazyPage"));

export function App() {
  return <Lazy />;
}
