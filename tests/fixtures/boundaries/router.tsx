import { createBrowserRouter } from "react-router-dom";
import { ErrorBoundary } from "./ErrorBoundary";
import { Home } from "./routes/Home";
import { PageA } from "./routes/PageA";
import { PageB } from "./routes/PageB";
import { PageC } from "./routes/PageC";
import { Oops } from "./routes/Oops";

export const router = createBrowserRouter([
  {
    path: "/",
    element: <Home />,
    errorElement: <Oops />,
    // child routes inherit the parent's errorElement
    children: [{ path: "a", element: <PageA /> }],
  },
  // no errorElement anywhere above it
  { path: "/b", element: <PageB /> },
  // wrapped in a boundary of its own
  { path: "/c", element: <ErrorBoundary><PageC /></ErrorBoundary> },
]);
