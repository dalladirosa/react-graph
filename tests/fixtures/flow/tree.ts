export function renderTree(node: any): string {
  if (!node.children) return node.label;
  return renderChildren(node.children);
}

export function renderChildren(kids: any[]): string {
  return kids.map((k) => renderTree(k)).join("");
}
