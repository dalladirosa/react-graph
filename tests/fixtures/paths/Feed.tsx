import { round } from "./math";

export function Feed() {
  return <Card price={3.14159} />;
}

export function Card({ price }: { price: number }) {
  return <b>{formatPrice(price)}</b>;
}

function formatPrice(n: number) {
  return round(n);
}
