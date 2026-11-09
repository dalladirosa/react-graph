import { Toolbar as BToolbar } from "../b/Toolbar";
import { Toolbar } from "./Toolbar";

export function PageA() {
  return (
    <>
      <Toolbar />
      <BToolbar onSave={() => {}} />
    </>
  );
}
