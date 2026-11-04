import { useMemo } from "react";
import { IconAdjustments } from "@tabler/icons-react";
import { Select } from "./Select";
import {
  allEqProfiles,
  resolveEqProfile,
  type EqBandType,
  type EqProfile,
} from "../lib/eq";
import { useApp } from "../state";

function savedAsProfiles(
  saved: {
    id: string;
    name: string;
    bands: {
      id: string;
      type: string;
      frequency: number;
      gain: number;
      q: number;
      enabled: boolean;
    }[];
    bypass: boolean;
  }[],
): EqProfile[] {
  return saved.map((p) => ({
    ...p,
    bands: p.bands.map((b) => ({ ...b, type: b.type as EqBandType })),
  }));
}

export function VoiceProfileSelect({
  applyToMedia,
  className,
  disabled,
}: {
  applyToMedia?: (profile: EqProfile) => void;
  className?: string;
  disabled?: boolean;
}) {
  const { settings, persistSettings } = useApp();
  const saved = useMemo(
    () => savedAsProfiles(settings.eq_profiles ?? []),
    [settings.eq_profiles],
  );
  const catalog = useMemo(() => allEqProfiles(saved), [saved]);
  const activeId = settings.active_eq_profile_id || "flat";

  return (
    <Select
      className={className}
      label="Voice profile"
      icon={<IconAdjustments size={18} stroke={1.75} />}
      value={activeId}
      disabled={disabled}
      onChange={(id) => {
        const next = resolveEqProfile(id, saved);
        applyToMedia?.(next);
        void persistSettings({ active_eq_profile_id: id }).catch(() => undefined);
      }}
      options={catalog.map((p) => ({
        value: p.id,
        label: p.builtin ? p.name : `${p.name} (saved)`,
      }))}
    />
  );
}
