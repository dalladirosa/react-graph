import { fetchJson } from "./api";

export function useProducts() {
  return fetchJson("/products");
}
