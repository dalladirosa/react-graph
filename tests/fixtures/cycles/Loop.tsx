// A -> B -> C -> A: a render loop
export function A() {
  return <B />;
}

export function B() {
  return <C />;
}

export function C() {
  return <A />;
}

// renders the loop from outside; not part of it
export function Entry() {
  return <A />;
}

// a tree rendering itself is plain recursion, not a cycle
export function Tree() {
  return (
    <ul>
      <Tree />
      <Leaf />
    </ul>
  );
}

export function Leaf() {
  return <li />;
}
