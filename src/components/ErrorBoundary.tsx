import { Component, type ErrorInfo, type ReactNode } from "react";

type Props = {
  children: ReactNode;
  label?: string;
};

type State = {
  error: Error | null;
};

/** Keeps a route from going fully blank when a child throws during render. */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(this.props.label ?? "ErrorBoundary", error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="rs-error-boundary">
          <h1>Something went wrong</h1>
          <p>{this.state.error.message || "This screen crashed while rendering."}</p>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => {
              this.setState({ error: null });
              window.location.hash = "";
              window.location.href = "/recordings";
            }}
          >
            Reload recordings
          </button>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => {
              this.setState({ error: null });
              window.location.href = "/";
            }}
          >
            Back to studio
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
