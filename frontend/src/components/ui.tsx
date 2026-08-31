/**
 * Small shared Tailwind building blocks so the status-coloring convention established in
 * the `frontend-shell` health indicator (slate = pending/neutral, red = error, emerald =
 * success) stays consistent across every screen rather than being re-invented per file.
 */
import { useEffect, useId, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

export function PageHeading({
  title,
  description,
  actions,
}: {
  readonly title: string;
  readonly description?: string;
  readonly actions?: ReactNode;
}) {
  return (
    <div className="mb-6 flex items-start justify-between gap-4">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        {description && (
          <p className="mt-1 text-sm text-slate-400">{description}</p>
        )}
      </div>
      {actions && <div className="flex shrink-0 gap-2">{actions}</div>}
    </div>
  );
}

const buttonVariants = {
  primary:
    "border-emerald-800 bg-emerald-900/40 text-emerald-300 hover:bg-emerald-900/70",
  danger: "border-red-800 bg-red-950 text-red-300 hover:bg-red-900/60",
  neutral: "border-slate-700 bg-slate-900 text-slate-200 hover:bg-slate-800",
} as const;

export function Button({
  children,
  onClick,
  type = "button",
  variant = "neutral",
  disabled = false,
  title,
  autoFocus = false,
}: {
  readonly children: ReactNode;
  readonly onClick?: () => void;
  readonly type?: "button" | "submit";
  readonly variant?: keyof typeof buttonVariants;
  readonly disabled?: boolean;
  readonly title?: string;
  readonly autoFocus?: boolean;
}) {
  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled}
      title={title}
      autoFocus={autoFocus}
      className={`rounded-md border px-3 py-1.5 text-sm font-medium disabled:cursor-not-allowed disabled:opacity-50 ${buttonVariants[variant]}`}
    >
      {children}
    </button>
  );
}

export function Card({ children }: { readonly children: ReactNode }) {
  return (
    <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-5">
      {children}
    </div>
  );
}

/** A reusable native details section with an accessible heading and disclosure indicator. */
export function CollapsibleSection({
  title,
  children,
  defaultOpen = true,
  trailing,
  className = "mb-6",
}: {
  readonly title: string;
  readonly children: ReactNode;
  readonly defaultOpen?: boolean;
  readonly trailing?: ReactNode;
  readonly className?: string;
}) {
  const [isOpen, setIsOpen] = useState(defaultOpen);

  return (
    <details
      className={`group ${className}`}
      open={isOpen}
      onToggle={(event) => setIsOpen(event.currentTarget.open)}
    >
      <summary className="mb-3 flex cursor-pointer list-none items-center justify-between gap-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        <h2>{title}</h2>
        <span className="flex shrink-0 items-center gap-3">
          {trailing}
          <span
            aria-hidden="true"
            className="text-base transition-transform group-open:rotate-90"
          >
            ▸
          </span>
        </span>
      </summary>
      {children}
    </details>
  );
}

export function ErrorBanner({ message }: { readonly message: string }) {
  return (
    <div
      role="alert"
      className="rounded-md border border-red-800 bg-red-950 px-4 py-3 text-sm text-red-300"
    >
      {message}
    </div>
  );
}

export function EmptyState({ message }: { readonly message: string }) {
  return (
    <div className="rounded-md border border-slate-800 bg-slate-900/40 px-4 py-8 text-center text-sm text-slate-400">
      {message}
    </div>
  );
}

export function LoadingState({
  message = "Loading…",
}: {
  readonly message?: string;
}) {
  return (
    <div className="rounded-md border border-slate-700 bg-slate-900 px-4 py-8 text-center text-sm text-slate-400">
      {message}
    </div>
  );
}

const badgeTones: Record<string, string> = {
  neutral: "border-slate-700 bg-slate-900 text-slate-300",
  success: "border-emerald-800 bg-emerald-950 text-emerald-300",
  error: "border-red-800 bg-red-950 text-red-300",
  warning: "border-amber-800 bg-amber-950 text-amber-300",
};

export function Badge({
  children,
  tone = "neutral",
}: {
  readonly children: ReactNode;
  readonly tone?: keyof typeof badgeTones;
}) {
  return (
    <span
      className={`inline-flex items-center rounded-md border px-2 py-0.5 font-mono text-xs ${badgeTones[tone]}`}
    >
      {children}
    </span>
  );
}

export function Section({
  title,
  children,
  collapsible = false,
  defaultOpen = true,
}: {
  readonly title: string;
  readonly children: ReactNode;
  readonly collapsible?: boolean;
  readonly defaultOpen?: boolean;
}) {
  const content = (
    <Card>
      <dl className="grid grid-cols-1 gap-x-6 gap-y-3 sm:grid-cols-2">
        {children}
      </dl>
    </Card>
  );

  if (collapsible) {
    return (
      <CollapsibleSection title={title} defaultOpen={defaultOpen}>
        {content}
      </CollapsibleSection>
    );
  }

  return (
    <section className="mb-6">
      <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        {title}
      </h2>
      {content}
    </section>
  );
}

/** One label/value pair inside a `<Section>`'s definition list. */
export function Field({
  label,
  value,
  full = false,
}: {
  readonly label: string;
  readonly value: ReactNode;
  readonly full?: boolean;
}) {
  return (
    <div className={full ? "sm:col-span-2" : undefined}>
      <dt className="text-xs uppercase tracking-wide text-slate-500">
        {label}
      </dt>
      <dd className="mt-0.5 font-mono text-sm text-slate-100 break-words">
        {value}
      </dd>
    </div>
  );
}

const fieldControlClass =
  "mt-1 w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 placeholder:text-slate-600 focus:border-slate-500 focus:outline-none";

/** A labeled `<input>` inside a form section, mirroring `<Field>`'s read-only counterpart. */
export function TextField({
  label,
  value,
  onChange,
  required = false,
  placeholder,
  full = false,
  hint,
  disabled = false,
}: {
  readonly label: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly required?: boolean;
  readonly placeholder?: string;
  readonly full?: boolean;
  readonly hint?: string;
  readonly disabled?: boolean;
}) {
  return (
    <label className={`block ${full ? "sm:col-span-2" : ""}`}>
      <span className="text-xs uppercase tracking-wide text-slate-500">
        {label}
        {required && <span className="ml-1 text-red-400">*</span>}
      </span>
      <input
        type="text"
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        className={`${fieldControlClass} disabled:cursor-not-allowed disabled:opacity-50`}
      />
      {hint && (
        <span className="mt-1 block text-xs text-slate-500">{hint}</span>
      )}
    </label>
  );
}

/** A labeled multiline text control for freeform run metadata such as operator notes. */
export function TextAreaField({
  label,
  value,
  onChange,
  placeholder,
  full = false,
  hint,
  rows = 4,
}: {
  readonly label: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly placeholder?: string;
  readonly full?: boolean;
  readonly hint?: string;
  readonly rows?: number;
}) {
  return (
    <label className={`block ${full ? "sm:col-span-2" : ""}`}>
      <span className="text-xs uppercase tracking-wide text-slate-500">
        {label}
      </span>
      <textarea
        value={value}
        placeholder={placeholder}
        rows={rows}
        onChange={(e) => onChange(e.target.value)}
        className={`${fieldControlClass} resize-y`}
      />
      {hint && (
        <span className="mt-1 block text-xs text-slate-500">{hint}</span>
      )}
    </label>
  );
}

/**
 * A labeled numeric `<input>`, mirroring `<TextField>` but using `type="number"` so the
 * browser offers native validation/steppers and `e.target.valueAsNumber` avoids re-parsing
 * strings. `value`/`onChange` use `""` (not `undefined`) for "left blank", since an
 * uncontrolled-vs-controlled `<input>` switch on `undefined` triggers a React warning.
 */
export function NumberField({
  label,
  value,
  onChange,
  required = false,
  placeholder,
  hint,
  step,
  min,
  max,
  full = false,
  disabled = false,
}: {
  readonly label: string;
  readonly value: number | "";
  readonly onChange: (value: number | "") => void;
  readonly required?: boolean;
  readonly placeholder?: string;
  readonly hint?: string;
  readonly step?: number | string;
  readonly min?: number;
  readonly max?: number;
  readonly full?: boolean;
  readonly disabled?: boolean;
}) {
  return (
    <label className={`block ${full ? "sm:col-span-2" : ""}`}>
      <span className="text-xs uppercase tracking-wide text-slate-500">
        {label}
        {required && <span className="ml-1 text-red-400">*</span>}
      </span>
      <input
        type="number"
        value={value}
        placeholder={placeholder}
        step={step}
        min={min}
        max={max}
        disabled={disabled}
        onChange={(e) =>
          onChange(e.target.value === "" ? "" : e.target.valueAsNumber)
        }
        className={`${fieldControlClass} disabled:cursor-not-allowed disabled:opacity-50`}
      />
      {hint && (
        <span className="mt-1 block text-xs text-slate-500">{hint}</span>
      )}
    </label>
  );
}

/**
 * A labeled `<select>` for a fixed set of enum options. `placeholder`, when given, renders a
 * leading `value=""` option with human-readable text (e.g. "Auto-detect") — for an optional
 * enum field where `T` includes `""` for "unset". `displayLabel`, when given, maps each raw
 * option value to human-readable text (e.g. `pressure_line` -> "Pressure (Line)", via one of
 * the maps in `lib/enumLabels`); omit it for genuinely free-form options (e.g. template
 * names) where the raw value already is the display text.
 */
export function SelectField<
  Value extends string,
  Option extends Value = Value,
>({
  label,
  value,
  onChange,
  options,
  full = false,
  placeholder,
  required = false,
  hint,
  disabled = false,
  displayLabel,
}: {
  readonly label: string;
  readonly value: Value;
  readonly onChange: (value: Value) => void;
  readonly options: readonly Option[];
  readonly full?: boolean;
  readonly placeholder?: string;
  readonly required?: boolean;
  readonly hint?: string;
  readonly disabled?: boolean;
  readonly displayLabel?: (value: Option) => string;
}) {
  return (
    <label className={`block ${full ? "sm:col-span-2" : ""}`}>
      <span className="text-xs uppercase tracking-wide text-slate-500">
        {label}
        {required && <span className="ml-1 text-red-400">*</span>}
      </span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value as Value)}
        disabled={disabled}
        className={`${fieldControlClass} disabled:cursor-not-allowed disabled:opacity-50`}
      >
        {placeholder !== undefined && <option value="">{placeholder}</option>}
        {options.map((option) => (
          <option key={option} value={option}>
            {displayLabel ? displayLabel(option) : option}
          </option>
        ))}
      </select>
      {hint && (
        <span className="mt-1 block text-xs text-slate-500">{hint}</span>
      )}
    </label>
  );
}

/** A labeled checkbox, styled to line up with `<TextField>`/`<SelectField>` in a grid. */
export function CheckboxField({
  label,
  checked,
  onChange,
  hint,
  disabled = false,
}: {
  readonly label: string;
  readonly checked: boolean;
  readonly onChange: (checked: boolean) => void;
  readonly hint?: string;
  readonly disabled?: boolean;
}) {
  return (
    <label className="flex items-start gap-2 pt-5">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        disabled={disabled}
        className="mt-0.5 h-4 w-4 rounded border-slate-700 bg-slate-950 disabled:cursor-not-allowed disabled:opacity-50"
      />
      <span className={disabled ? "opacity-50" : undefined}>
        <span className="block text-sm text-slate-200">{label}</span>
        {hint && <span className="block text-xs text-slate-500">{hint}</span>}
      </span>
    </label>
  );
}

export function FormSection({
  title,
  children,
  collapsible = false,
  defaultOpen = false,
}: {
  readonly title: string;
  readonly children: ReactNode;
  readonly collapsible?: boolean;
  readonly defaultOpen?: boolean;
}) {
  const content = (
    <Card>
      <div className="grid grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-2">
        {children}
      </div>
    </Card>
  );

  if (collapsible) {
    return (
      <CollapsibleSection title={title} defaultOpen={defaultOpen}>
        {content}
      </CollapsibleSection>
    );
  }

  return (
    <section className="mb-6">
      <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        {title}
      </h2>
      {content}
    </section>
  );
}

/**
 * A centered viewport popup: a full-viewport backdrop behind a bordered panel, following the
 * same dark `slate` theme as every other component here. First built for the OPC tag-tree
 * browser (`ui-opc-browser`) and the run-detail PID action review flow. Rendering through a
 * document-body portal keeps the popup independent of the page's scroll position and layout
 * containers. Closes on a backdrop click, the header's close button, or Escape unless
 * `dismissible` is false. The backdrop is a native button behind the dialog panel, so it does
 * not interfere with controls inside the panel.
 */
export function Modal({
  title,
  onClose,
  children,
  widthClassName = "max-w-lg",
  dismissible = true,
}: {
  readonly title: string;
  readonly onClose: () => void;
  readonly children: ReactNode;
  readonly widthClassName?: string;
  readonly dismissible?: boolean;
}) {
  const titleId = useId();

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (dismissible && event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [dismissible, onClose]);

  useEffect(() => {
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = previousOverflow;
    };
  }, []);

  return createPortal(
    <div className="modal-backdrop fixed inset-0 z-50 flex items-center justify-center overflow-y-auto p-4">
      <button
        type="button"
        aria-label="Dismiss modal backdrop"
        onClick={onClose}
        disabled={!dismissible}
        className="absolute inset-0 cursor-default disabled:cursor-not-allowed"
      />
      <dialog
        open
        aria-modal="true"
        aria-labelledby={titleId}
        className={`relative z-10 max-h-[calc(100vh-2rem)] w-full ${widthClassName} overflow-hidden rounded-lg border border-slate-700 bg-slate-900 shadow-xl`}
      >
        <div className="flex items-center justify-between border-b border-slate-800 px-4 py-3">
          <h2 id={titleId} className="text-sm font-semibold text-slate-200">
            {title}
          </h2>
          <button
            type="button"
            onClick={onClose}
            disabled={!dismissible}
            aria-label="Close"
            title={
              !dismissible ? "Finish the current operation first" : undefined
            }
            className="text-slate-400 hover:text-slate-200 disabled:cursor-not-allowed disabled:opacity-50"
          >
            ✕
          </button>
        </div>
        <div className="max-h-[70vh] overflow-y-auto p-4">{children}</div>
      </dialog>
    </div>,
    document.body,
  );
}
