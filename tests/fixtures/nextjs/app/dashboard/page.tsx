import { useState } from "react";

// interactive, but nothing says "use client"
export default function Dashboard() {
  const [open] = useState(false);
  return <button onClick={() => {}}>{String(open)}</button>;
}
