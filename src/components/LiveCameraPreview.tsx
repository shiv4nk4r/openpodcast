import { useEffect, useRef } from "react";

/**
 * Live camera preview that never shows WKWebView/Safari play-pause chrome.
 * The real <video> stays off-screen; frames are mirrored onto a <canvas>.
 */
export function LiveCameraPreview({
  setVideoEl,
  className,
  mirror = true,
}: {
  setVideoEl: (el: HTMLVideoElement | null) => void;
  className?: string;
  mirror?: boolean;
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

    const tick = () => {
      const v = videoRef.current;
      const c = canvasRef.current;
      if (v && c && v.readyState >= 2 && v.videoWidth > 0) {
        const rect = c.getBoundingClientRect();
        const cssW = Math.max(1, Math.round(rect.width * devicePixelRatio));
        const cssH = Math.max(1, Math.round(rect.height * devicePixelRatio));
        if (c.width !== cssW || c.height !== cssH) {
          c.width = cssW;
          c.height = cssH;
        }
        const ctx = c.getContext("2d");
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
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(rafRef.current);
      video.removeEventListener("pause", onPause);
    };
  }, [mirror]);

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
