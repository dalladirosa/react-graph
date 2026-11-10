import { lazy, Suspense, type ReactNode } from "react";

const Reports = lazy(() => import("./pages/Reports"));
const Billing = lazy(() => import("./pages/Billing"));
const Charts = lazy(() => import("./pages/Charts"));
const Help = lazy(() => import("./pages/Help"));

function Spinner() {
  return <i />;
}

// renders its children inside a Suspense: whatever it wraps is suspended
function Shell({ children }: { children: ReactNode }) {
  return <Suspense fallback={<Spinner />}>{children}</Suspense>;
}

export function App() {
  return (
    <div>
      <Suspense fallback={<Spinner />}>
        <Reports />
      </Suspense>
      <Billing />
      <Shell>
        <Charts />
      </Shell>
      <Suspense fallback={null}>
        <Help />
      </Suspense>
      <Help />
    </div>
  );
}
