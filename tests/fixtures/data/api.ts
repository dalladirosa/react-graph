import axios from "axios";

export async function getProducts() {
  const res = await fetch("/api/products");
  return res.json();
}

export function createOrder(order: { id: string }) {
  return fetch(`/api/orders/${order.id}`, { method: "post", body: JSON.stringify(order) });
}

export const loadUser = (id: string) => axios.get(`/api/users/${id}`);

export function lookup(url: string) {
  return fetch(url);
}

// a Map is not an HTTP client
export function cached(map: Map<string, string>) {
  return map.get("key");
}
