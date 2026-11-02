import { useEffect, useRef } from "react";

/**
 * Live camera preview that never shows WKWebView/Safari play-pause chrome.
 * The real <video> stays off-screen; frames are mirrored onto a <canvas>.
 */
export function LiveCameraPreview({
  setVideoEl,
  className,
  mirror = true,
  /** Cap paint rate to cut CPU while recording (default ~30fps). */
  maxFps = 30,
}: {
  setVideoEl: (el: HTMLVideoElement | null) => void;
  className?: string;
  mirror?: boolean;
  maxFps?: number;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const rafRef = useRef(0);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    video.controls = false;
    video.muted = true;
    video.playsInline = true;
    video.disablePictureInPicture = true;
    video.setAttribute("playsinline", "true");
    video.setAttribute("webkit-playsinline", "true");
    video.setAttribute("controlslist", "nodownload nofullscreen noremoteplayback");

    const onPause = () => {
      if (video.paused) void video.play().catch(() => undefined);
    };
    video.addEventListener("pause", onPause);

    const minFrameMs = 1000 / Math.max(5, Math.min(60, maxFps));
    let lastPaint = 0;

    const tick = (now: number) => {
      const v = videoRef.current;
      const c = canvasRef.current;
      if (v && c && v.readyState >= 2 && v.videoWidth > 0) {
        if (now - lastPaint >= minFrameMs) {
          lastPaint = now;
          // Cap canvas resolution — preview doesn't need full 1080p@2x
          const rect = c.getBoundingClientRect();
          const dpr = Math.min(1.25, devicePixelRatio || 1);
          const cssW = Math.max(1, Math.round(rect.width * dpr));
          const cssH = Math.max(1, Math.round(rect.height * dpr));
          if (c.width !== cssW || c.height !== cssH) {
            c.width = cssW;
            c.height = cssH;
          }
          const ctx = c.getContext("2d", { alpha: false });
          if (ctx) {
            // object-fit: cover
            const scale = Math.max(cssW / v.videoWidth, cssH / v.videoHeight);
            const dw = v.videoWidth * scale;
            const dh = v.videoHeight * scale;
            const dx = (cssW - dw) / 2;
            const dy = (cssH - dh) / 2;
            ctx.save();
            if (mirror) {
              ctx.setTransform(-1, 0, 0, 1, cssW, 0);
            } else {
              ctx.setTransform(1, 0, 0, 1, 0, 0);
            }
            ctx.drawImage(v, dx, dy, dw, dh);
            ctx.restore();
          }
        }
      }
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(rafRef.current);
      video.removeEventListener("pause", onPause);
    };
  }, [mirror, maxFps]);

  return (
    <>
      <video
        ref={(el) => {
          videoRef.current = el;
          setVideoEl(el);
        }}
        className="rs-live-video-hidden"
        muted
        playsInline
        autoPlay
        disablePictureInPicture
        disableRemotePlayback
        controls={false}
      />
      <canvas ref={canvasRef} className={className} aria-hidden />
    </>
  );
}
