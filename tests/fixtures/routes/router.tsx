import { createBrowserRouter } from "react-router-dom";
import { Layout } from "./Layout";
import { Home } from "./pages/Home";
import { About } from "./pages/About";
import { NotFound } from "./pages/NotFound";
import { Settings } from "./pages/Settings";

// the pages are only referenced here, at module level: no component renders them
export const router = createBrowserRouter([
  {
    path: "/",
    element: <Layout />,
    errorElement: <NotFound />,
    children: [
      { index: true, element: <Home /> },
      { path: "about", element: <About /> },
      { path: "settings", Component: Settings },
    ],
  },
]);

// not a route config: no `element` / `Component` entries
export const theme = { color: "red", size: 12 };
