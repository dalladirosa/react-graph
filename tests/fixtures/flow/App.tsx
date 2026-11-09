import { Shop } from "./Shop";
import { CartTotal } from "./CartTotal";

export function App() {
  return (
    <main>
      <Shop />
      <CartTotal total={0} />
    </main>
  );
}
