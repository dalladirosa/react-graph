export function Card({ title }: { title: string }) {
  function onClick() {}
  return <div onClick={onClick}>{title}</div>;
}

export function CardSkeleton() {
  return <div className="skeleton" />;
}
