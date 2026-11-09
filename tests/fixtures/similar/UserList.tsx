import { useState } from "react";

type User = { id: string; name: string; email: string };

// ProductList plus an empty state and a counter: similar, not identical
export function UserList({ users, onOpen }: { users: User[]; onOpen: (id: string) => void }) {
  const [query, setQuery] = useState("");
  const visible = users.filter((u) => u.name.toLowerCase().includes(query.toLowerCase()));
  const close = () => setQuery("");
  return (
    <div className="list">
      <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search users" />
      <button onClick={close}>Clear</button>
      <ul>
        {visible.map((u) => (
          <li key={u.id} onClick={() => onOpen(u.id)}>
            {u.name} — {u.email}
          </li>
        ))}
      </ul>
      <p className="count">{visible.length} users</p>
    </div>
  );
}
