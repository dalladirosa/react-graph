import { useState } from "react";
import { Layout } from "./Chain";
import { Shallow } from "./Shallow";
import { Dashboard } from "./Dashboard";
import { Toolbar } from "./Toolbar";

export function App() {
  const [user] = useState({ name: "Ada" });
  return (
    <>
      <Layout user={user} theme="dark" />
      <Shallow label="x" />
      <Dashboard />
      <Toolbar title="Home" />
    </>
  );
}
