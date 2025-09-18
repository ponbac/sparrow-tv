import React, { useRef, useState, useEffect } from "react";
import {
  Play,
  Pause,
  Volume2,
  VolumeX,
  Maximize,
  Minimize,
  X,
} from "lucide-react";
import mpegts from "mpegts.js";
import { cn } from "@/lib/utils";

const INITIAL_VOLUME = 0.25;
const MAX_RETRIES = 5;
const INITIAL_BACKOFF_MS = 1000;
const MAX_BACKOFF_MS = 30000;
const STALL_THRESHOLD_MS = 8000; // consider frozen if no progress for 8s

export const TvPlayer = (props: { url: string; onClose?: () => void }) => {
  const [isPlaying, setIsPlaying] = useState(false);
  const [isMuted, setIsMuted] = useState(false);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [volume, setVolume] = useState(INITIAL_VOLUME);

  const videoRef = useRef<HTMLVideoElement>(null);
  const playerRef = useRef<mpegts.Player | null>(null);
  const reconnectAttemptsRef = useRef(0);
  const backoffTimeoutRef = useRef<number | null>(null);
  const stallIntervalRef = useRef<number | null>(null);
  const lastPlaybackTimeRef = useRef(0);
  const lastProgressAtRef = useRef<number>(Date.now());
  // Backoff timer presence is used to dedupe reconnect attempts

  const togglePlay = () => {
    if (videoRef.current) {
      if (isPlaying) {
        videoRef.current.pause();
      } else {
        videoRef.current.play();
      }
      setIsPlaying(!isPlaying);
    }
  };

  const toggleMute = () => {
    if (videoRef.current) {
      videoRef.current.muted = !isMuted;
      setIsMuted(!isMuted);
    }
  };

  const handleVolumeChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVolume = parseFloat(e.target.value);
    if (videoRef.current) {
      videoRef.current.volume = newVolume;
      setVolume(newVolume);
      setIsMuted(newVolume === 0);
    }
  };

  const toggleFullscreen = () => {
    if (!document.fullscreenElement && videoRef.current) {
      videoRef.current.requestFullscreen();
      setIsFullscreen(true);
    } else {
      document.exitFullscreen();
      setIsFullscreen(false);
    }
  };

  // Helper to clean up player and timers
  const destroyPlayer = () => {
    if (playerRef.current) {
      try {
        playerRef.current.destroy();
      } catch (e) {
        console.error("Failed to destroy player", e);
      }
      playerRef.current = null;
    }
  };

  const clearTimers = () => {
    if (backoffTimeoutRef.current) {
      window.clearTimeout(backoffTimeoutRef.current);
      backoffTimeoutRef.current = null;
    }
    if (stallIntervalRef.current) {
      window.clearInterval(stallIntervalRef.current);
      stallIntervalRef.current = null;
    }
  };

  const createPlayer = () => {
    if (!videoRef.current) return;
    if (!mpegts.getFeatureList().mseLivePlayback) return;

    const player = mpegts.createPlayer({
      type: "mpegts",
      url: props.url,
      isLive: true,
    });

    player.attachMediaElement(videoRef.current);
    videoRef.current.volume = volume ?? INITIAL_VOLUME;
    videoRef.current.muted = isMuted;

    // Reset progress trackers; keep retry attempts until we see progress
    lastPlaybackTimeRef.current = 0;
    lastProgressAtRef.current = Date.now();

    // Wire player error handling
    player.on(mpegts.Events.ERROR, () => {
      scheduleReconnect("mpegts error");
    });

    player.load();
    player.play();
    playerRef.current = player;

    // Start a stall watchdog
    if (stallIntervalRef.current) {
      window.clearInterval(stallIntervalRef.current);
    }
    stallIntervalRef.current = window.setInterval(() => {
      const video = videoRef.current;
      if (!video) return;
      const now = Date.now();

      const isLikelyStalled =
        !video.paused && now - lastProgressAtRef.current > STALL_THRESHOLD_MS;

      if (isLikelyStalled) {
        scheduleReconnect("stall watchdog");
      }
    }, 1000);
  };

  const scheduleReconnect = (reason: string) => {
    const attempts = reconnectAttemptsRef.current;
    if (attempts >= MAX_RETRIES) {
      console.error("Max reconnect attempts reached; giving up", { reason });
      return;
    }
    // If a reconnect timer is already pending, don't schedule another
    if (backoffTimeoutRef.current) return;

    const delay =
      Math.min(MAX_BACKOFF_MS, INITIAL_BACKOFF_MS * Math.pow(2, attempts)) +
      Math.floor(Math.random() * 250);

    reconnectAttemptsRef.current = attempts + 1;
    console.warn(`Reconnecting (attempt ${attempts + 1}) in ${delay}ms`, {
      reason,
    });

    if (backoffTimeoutRef.current) {
      window.clearTimeout(backoffTimeoutRef.current);
    }
    backoffTimeoutRef.current = window.setTimeout(() => {
      destroyPlayer();
      createPlayer();
      // Allow future retries if still stalled
      backoffTimeoutRef.current = null;
    }, delay);
  };

  useEffect(() => {
    if (!playerRef.current) {
      createPlayer();
    }

    return () => {
      clearTimers();
      destroyPlayer();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.url]);

  // Handle orientation change on mobile, set fullscreen if landscape
  useEffect(() => {
    function handleOrientationChange() {
      const isLandscape = screen.orientation.type.includes("landscape");

      if (isLandscape && videoRef.current && !document.fullscreenElement) {
        videoRef.current.requestFullscreen();
        setIsFullscreen(true);
      }
    }

    screen.orientation.addEventListener("change", handleOrientationChange);

    // Check initial orientation
    handleOrientationChange();

    return () => {
      screen.orientation.removeEventListener("change", handleOrientationChange);
    };
  }, []);

  return (
    <div className="fixed bottom-8 right-8 w-full max-w-4xl bg-gray-900 rounded-lg overflow-hidden shadow-2xl">
      <div className="relative">
        {props.onClose && (
          <button
            onClick={props.onClose}
            className="absolute top-2 right-2 z-10 text-white hover:text-red-400 transition-colors"
          >
            <X className="w-6 h-6" />
          </button>
        )}

        <video
          ref={videoRef}
          className="w-full aspect-video"
          onPlay={() => setIsPlaying(true)}
          onPause={() => setIsPlaying(false)}
          onTimeUpdate={() => {
            const video = videoRef.current;
            if (!video) return;
            const t = video.currentTime;
            if (t > lastPlaybackTimeRef.current + 0.05) {
              lastPlaybackTimeRef.current = t;
              lastProgressAtRef.current = Date.now();
              // Reset attempts on healthy progress
              reconnectAttemptsRef.current = 0;
            }
          }}
          onStalled={() => scheduleReconnect("video stalled event")}
          onWaiting={() => scheduleReconnect("video waiting event")}
          onError={(e) => {
            console.error("Video error:", e);
            scheduleReconnect("video error event");
          }}
        >
          <source src={props.url} type="application/x-mpegURL" />
          Your browser does not support the video tag.
        </video>

        <div className="absolute bottom-0 left-0 right-0 bg-gradient-to-t from-black/80 to-transparent p-4">
          <div className="flex items-center gap-4">
            <PlayButton isPlaying={isPlaying} onClick={togglePlay} />
            <VolumeHandler
              isMuted={isMuted}
              onToggleMute={toggleMute}
              volume={volume}
              onVolumeChange={handleVolumeChange}
            />
            <FullscreenButton
              isFullscreen={isFullscreen}
              onClick={toggleFullscreen}
              className="ml-auto"
            />
          </div>
        </div>
      </div>
    </div>
  );
};

function PlayButton(props: { isPlaying: boolean; onClick: () => void }) {
  return (
    <button
      onClick={props.onClick}
      className="text-white hover:text-blue-400 transition-colors"
    >
      {props.isPlaying ? (
        <Pause className="w-6 h-6" />
      ) : (
        <Play className="w-6 h-6" />
      )}
    </button>
  );
}

function VolumeHandler(props: {
  isMuted: boolean;
  onToggleMute: () => void;
  volume: number;
  onVolumeChange: (e: React.ChangeEvent<HTMLInputElement>) => void;
}) {
  return (
    <div className="flex items-center gap-2">
      <button
        onClick={props.onToggleMute}
        className="text-white hover:text-blue-400 transition-colors"
      >
        {props.isMuted ? (
          <VolumeX className="w-6 h-6" />
        ) : (
          <Volume2 className="w-6 h-6" />
        )}
      </button>
      <input
        type="range"
        min="0"
        max="1"
        step="0.1"
        value={props.volume}
        onChange={props.onVolumeChange}
        className="w-24 accent-blue-500"
      />
    </div>
  );
}

function FullscreenButton(props: {
  isFullscreen: boolean;
  onClick: () => void;
  className?: string;
}) {
  return (
    <button
      onClick={props.onClick}
      className={cn(
        "text-white hover:text-blue-400 transition-colors",
        props.className
      )}
    >
      {props.isFullscreen ? (
        <Minimize className="w-6 h-6" />
      ) : (
        <Maximize className="w-6 h-6" />
      )}
    </button>
  );
}
