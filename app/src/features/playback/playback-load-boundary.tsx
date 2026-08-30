import { Component, type ReactNode } from "react";

interface PlaybackLoadBoundaryProps {
  readonly children: ReactNode;
  readonly resetKey: string;
  readonly onStop: () => void;
  readonly onReload: () => void;
}

interface PlaybackLoadBoundaryState {
  readonly failed: boolean;
}

/** Keeps a failed lazy player chunk from taking the catalog down with it. */
export class PlaybackLoadBoundary extends Component<
  PlaybackLoadBoundaryProps,
  PlaybackLoadBoundaryState
> {
  state: PlaybackLoadBoundaryState = { failed: false };

  static getDerivedStateFromError(): PlaybackLoadBoundaryState {
    return { failed: true };
  }

  componentDidUpdate(previous: PlaybackLoadBoundaryProps) {
    if (previous.resetKey !== this.props.resetKey && this.state.failed) {
      this.setState({ failed: false });
    }
  }

  render() {
    if (!this.state.failed) {
      return this.props.children;
    }

    return (
      <section className="error-notice" role="alert">
        <div>
          <p className="eyebrow">Player module unavailable</p>
          <h2>The live player could not be loaded</h2>
          <p>
            Browsing remains available. Restore this connection, then reload
            Sparrow before selecting the Channel again.
          </p>
        </div>
        <div className="playback-load-actions">
          <button type="button" onClick={this.props.onStop}>
            Close player
          </button>
          <button type="button" onClick={this.props.onReload}>
            Reload Sparrow
          </button>
        </div>
      </section>
    );
  }
}
