import { AgentSession } from "./session";

export function App() {
  const session = new AgentSession();
  return <div>{String(session)}</div>;
}
