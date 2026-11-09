import { useState } from "react";

export const Dash = () => {
  const [items] = useState<number[]>([]);
  return <Panel items={items} />;
};

export const Panel = (props: any) => <Table items={props.items} />;

export const Table = (props: { items: number[] }) => {
  return <Row items={props.items} />;
};

export const Row = ({ items }: { items: number[] }) => <ul>{items.length}</ul>;
