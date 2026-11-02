export const RESOLUTION_OPTIONS = [
  { value: "720p", label: "720p", description: "1280×720 · lighter files" },
  { value: "1080p", label: "1080p", description: "1920×1080 · recommended" },
  { value: "1440p", label: "1440p", description: "2560×1440 · sharper" },
  { value: "2160p", label: "4K", description: "3840×2160 · largest files" },
] as const;

export const FRAME_RATE_OPTIONS = [
  { value: "24", label: "24 fps", description: "Cinematic" },
  { value: "25", label: "25 fps", description: "PAL / broadcast" },
  { value: "30", label: "30 fps", description: "Default for most podcasts" },
  { value: "50", label: "50 fps", description: "Smooth (PAL)" },
  { value: "60", label: "60 fps", description: "Extra smooth motion" },
] as const;

export function resolutionToSize(resolution: string): {
  width: number;
  height: number;
} {
  switch (resolution) {
    case "720p":
      return { width: 1280, height: 720 };
    case "1440p":
      return { width: 2560, height: 1440 };
    case "2160p":
    case "4K":
      return { width: 3840, height: 2160 };
    case "1080p":
    default:
      return { width: 1920, height: 1080 };
  }
}
