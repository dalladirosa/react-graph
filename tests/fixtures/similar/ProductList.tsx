import { useState } from "react";

type Product = { id: string; name: string; price: number };

export function ProductList({ items, onPick }: { items: Product[]; onPick: (id: string) => void }) {
  const [query, setQuery] = useState("");
  const visible = items.filter((p) => p.name.toLowerCase().includes(query.toLowerCase()));
  const close = () => setQuery("");
  return (
    <div className="list">
      <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search products" />
      <button onClick={close}>Clear</button>
      <ul>
        {visible.map((p) => (
          <li key={p.id} onClick={() => onPick(p.id)}>
            {p.name} — {p.price}
          </li>
        ))}
      </ul>
    </div>
  );
}

export function formatPrice(value: number, currency: string) {
  const rounded = Math.round(value * 100) / 100;
  const text = rounded.toFixed(2);
  if (currency === "JPY") return `¥${Math.round(rounded)}`;
  return `${currency} ${text}`;
}
