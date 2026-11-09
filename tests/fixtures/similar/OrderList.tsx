import { useState } from "react";

type Order = { id: string; ref: string; total: number };

// copy of ProductList with names and strings changed: identical structure
export function OrderList({ orders, onSelect }: { orders: Order[]; onSelect: (id: string) => void }) {
  const [filter, setFilter] = useState("");
  const shown = orders.filter((o) => o.ref.toLowerCase().includes(filter.toLowerCase()));
  const reset = () => setFilter("");
  return (
    <div className="orders">
      <input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Search orders" />
      <button onClick={reset}>Reset</button>
      <ul>
        {shown.map((o) => (
          <li key={o.id} onClick={() => onSelect(o.id)}>
            {o.ref} — {o.total}
          </li>
        ))}
      </ul>
    </div>
  );
}

// same body as formatPrice, written as an arrow function
export const formatTotal = (amount: number, code: string) => {
  const rounded = Math.round(amount * 100) / 100;
  const text = rounded.toFixed(2);
  if (code === "JPY") return `¥${Math.round(rounded)}`;
  return `${code} ${text}`;
};
