// Toolbar reads `title` itself, so it is not a pure pass-through
export function Toolbar({ title }: any) {
  return (
    <header>
      <h1>{title}</h1>
      <Menu title={title} />
    </header>
  );
}

export function Menu({ title }: any) {
  return <Item title={title} />;
}

export function Item({ title }: any) {
  return <li>{title}</li>;
}
