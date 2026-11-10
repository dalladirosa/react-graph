import { Logo } from "./Logo";

// reaches Logo in two steps through Legal, but Header gets there in one
export function Footer() {
  return <Legal />;
}

function Legal() {
  return <Logo />;
}
