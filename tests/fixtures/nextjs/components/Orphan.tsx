import { useState } from "react";

// nobody renders it from app/, and it has no directive
export function Orphan() {
  const [n] = useState(0);
  return <button onClick={() => {}}>{n}</button>;
}
