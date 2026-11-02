import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import {
  IconCheck,
  IconChevronDown,
  IconCircleFilled,
} from "@tabler/icons-react";

export interface SelectOption {
  value: string;
  label: string;
  description?: string;
  disabled?: boolean;
}

interface BaseProps {
  label?: string;
  hint?: string;
  options: SelectOption[];
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  emptyLabel?: string;
  /** Leading icon in the trigger (e.g. camera / mic / speaker). */
  icon?: ReactNode;
}

interface SingleProps extends BaseProps {
  multiple?: false;
  value: string;
  onChange: (value: string) => void;
}

interface MultiProps extends BaseProps {
  multiple: true;
  value: string[];
  onChange: (value: string[]) => void;
}

type Props = SingleProps | MultiProps;

function isMulti(props: Props): props is MultiProps {
  return props.multiple === true;
}

export function Select(props: Props) {
  const {
    label,
    hint,
    options,
    placeholder = "Select…",
    disabled,
    className,
    emptyLabel = "No options",
    icon,
  } = props;
  const listId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);

  const selectedValues = useMemo(() => {
    if (isMulti(props)) return props.value;
    return props.value ? [props.value] : [];
  }, [props]);

  const selectedLabels = options
    .filter((o) => selectedValues.includes(o.value))
    .map((o) => o.label);

  const triggerText =
    selectedLabels.length === 0
      ? placeholder
      : isMulti(props) && selectedLabels.length > 1
        ? `${selectedLabels[0]} +${selectedLabels.length - 1}`
        : selectedLabels.join(", ");

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const idx = Math.max(
      0,
      options.findIndex((o) => selectedValues.includes(o.value)),
    );
    setActive(idx === -1 ? 0 : idx);
  }, [open, options, selectedValues]);

  function toggleOption(value: string) {
    if (isMulti(props)) {
      const next = selectedValues.includes(value)
        ? selectedValues.filter((v) => v !== value)
        : [...selectedValues, value];
      props.onChange(next);
    } else {
      props.onChange(value);
      setOpen(false);
    }
  }

  function onTriggerKey(e: KeyboardEvent) {
    if (disabled) return;
    if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
      e.preventDefault();
      setOpen(true);
    }
  }

  function onListKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(options.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(0, i - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const opt = options[active];
      if (opt && !opt.disabled) toggleOption(opt.value);
    } else if (e.key === "Escape") {
      setOpen(false);
    }
  }

  return (
    <div
      className={`op-field ${className ?? ""}`}
      ref={rootRef}
      data-open={open ? "true" : "false"}
    >
      {label && <span className="op-field-label">{label}</span>}
      <button
        type="button"
        className="op-select-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={listId}
        disabled={disabled || options.length === 0}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={onTriggerKey}
      >
        {icon && <span className="op-select-leading">{icon}</span>}
        <span className={selectedLabels.length ? "op-select-value" : "op-select-placeholder"}>
          {options.length === 0 ? emptyLabel : triggerText}
        </span>
        <span className="op-select-chevron" aria-hidden>
          <IconChevronDown size={16} stroke={2} />
        </span>
      </button>
      {hint && <span className="op-field-hint">{hint}</span>}
      {open && options.length > 0 && (
        <div
          className="op-select-menu"
          role="listbox"
          id={listId}
          tabIndex={-1}
          aria-multiselectable={isMulti(props) || undefined}
          onKeyDown={onListKey}
        >
          {options.map((opt, i) => {
            const selected = selectedValues.includes(opt.value);
            return (
              <button
                key={opt.value}
                type="button"
                role="option"
                aria-selected={selected}
                disabled={opt.disabled}
                className={`op-select-option ${selected ? "is-selected" : ""} ${
                  i === active ? "is-active" : ""
                }`}
                onMouseEnter={() => setActive(i)}
                onClick={() => toggleOption(opt.value)}
              >
                <span className={`op-select-check ${isMulti(props) ? "is-multi" : ""}`}>
                  {selected ? (
                    isMulti(props) ? (
                      <IconCheck size={14} stroke={2.5} />
                    ) : (
                      <IconCircleFilled size={8} />
                    )
                  ) : null}
                </span>
                <span className="op-select-option-text">
                  <span className="op-select-option-label">{opt.label}</span>
                  {opt.description && (
                    <span className="op-select-option-desc">{opt.description}</span>
                  )}
                </span>
              </button>
            );
          })}
          {isMulti(props) && selectedValues.length > 0 && (
            <div className="op-select-footer">
              <button
                type="button"
                className="op-select-clear"
                onClick={() => props.onChange([])}
              >
                Clear
              </button>
              <button
                type="button"
                className="btn btn-primary op-select-done"
                onClick={() => setOpen(false)}
              >
                Done
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export function Segmented({
  label,
  value,
  onChange,
  options,
}: {
  label?: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: ReactNode }[];
}) {
  return (
    <div className="op-field">
      {label && <span className="op-field-label">{label}</span>}
      <div className="segmented" role="tablist">
        {options.map((o) => (
          <button
            key={o.value}
            type="button"
            role="tab"
            aria-selected={value === o.value}
            className={value === o.value ? "active" : ""}
            onClick={() => onChange(o.value)}
          >
            {o.label}
          </button>
        ))}
      </div>
    </div>
  );
}
