// one level of forwarding is normal
export function Shallow({ label }: any) {
  return <Badge label={label} />;
}

export function Badge({ label }: any) {
  return <b>{label}</b>;
}
