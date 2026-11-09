import { Toolbar } from "./Toolbar";
import { useThing } from "./useThing";

export function PageB() {
  const save = () => useThing();
  return <Toolbar onSave={save} />;
}
