"use client";
import { useState } from "react";
import { Label } from "./Label";

export function Counter() {
  const [n, setN] = useState(0);
  return (
    <button onClick={() => setN(n + 1)}>
      <Label text={String(n)} />
    </button>
  );
}
