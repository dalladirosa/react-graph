import { RouterProvider } from "react-router-dom";
import { router } from "./router";

// module-level JSX outside any config: left alone
render(<RouterProvider router={router} />);
