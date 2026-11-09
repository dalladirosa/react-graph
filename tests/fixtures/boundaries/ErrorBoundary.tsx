import { Component } from "react";

export class ErrorBoundary extends Component<{ children: any }> {
  render() {
    return this.props.children;
  }
}
