import { useState } from "react";

// the same chain written with a `props` object
export function Dashboard() {
  const [items] = useState<number[]>([]);
  return <Panel items={items} />;
}

export function Panel(props: any) {
  return <Table items={props.items} />;
}

export function Table(props: any) {
  return <Row items={props.items} />;
}

export function Row({ items }: any) {
  return <ul>{items.length}</ul>;
}
