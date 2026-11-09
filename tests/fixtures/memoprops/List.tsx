import { useCallback, useMemo } from "react";
import { Card } from "./Card";
import { Plain } from "./Plain";

export function List({ items }: { items: any[] }) {
  const style = { color: "red" }; // new object every render
  const select = () => {}; // new function every render
  const stable = useCallback(() => {}, []);
  const memoStyle = useMemo(() => ({ color: "blue" }), []);
  return (
    <div>
      <Card item={items[0]} onSelect={() => {}} />
      <Card item={items[1]} style={style} />
      <Card item={items[2]} onSelect={select} />
      <Card item={items[3]} onSelect={stable} style={memoStyle} />
      <Card item={items[4]} options={[1, 2]} />
      <Plain onSelect={() => {}} style={{}} />
    </div>
  );
}
