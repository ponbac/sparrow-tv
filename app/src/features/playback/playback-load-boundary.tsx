import { Component, type ReactNode } from "react";
import "./playback-load-boundary.css";

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
      <section className="playback-load-notice" role="alert">
        <h2>The player could not load</h2>
        <p>
          You can keep browsing. Check the connection, then reload Sparrow and
          choose the channel again.
        </p>
        <div className="playback-load-notice__actions">
          <button type="button" onClick={this.props.onReload}>
            Reload Sparrow
          </button>
          <button type="button" onClick={this.props.onStop}>
            Close player
          </button>
        </div>
      </section>
    );
  }
}
