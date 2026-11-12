import { formatTokens, readRunUsage } from "./usage";

export class AgentSession {
  private applyUsage(value: unknown) {
    const usage = readRunUsage(value);
    return usage;
  }

  label = () => formatTokens(3);
}

export class Orphan {
  run() {
    return 1;
  }
}

export abstract class Base {
  run() {
    return formatTokens(1);
  }
}
