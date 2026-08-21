/**
 * Small shared Tailwind building blocks so the status-coloring convention established in
 * the `frontend-shell` health badge (slate = pending/neutral, red = error, emerald =
 * success) stays consistent across every screen rather than being re-invented per file.
 */
import { useEffect, type ReactNode } from "react";

export function PageHeading({
  title,
  description,
  actions,
}: {
  title: string;
  description?: string;
  actions?: ReactNode;
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
}: {
  children: ReactNode;
  onClick?: () => void;
  type?: "button" | "submit";
  variant?: keyof typeof buttonVariants;
  disabled?: boolean;
  title?: string;
}) {
  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={`rounded-md border px-3 py-1.5 text-sm font-medium disabled:cursor-not-allowed disabled:opacity-50 ${buttonVariants[variant]}`}
    >
      {children}
    </button>
  );
}

export function Card({ children }: { children: ReactNode }) {
  return (
    <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-5">
      {children}
    </div>
  );
}

export function ErrorBanner({ message }: { message: string }) {
  return (
    <div className="rounded-md border border-red-800 bg-red-950 px-4 py-3 text-sm text-red-300">
      {message}
    </div>
  );
}

export function EmptyState({ message }: { message: string }) {
  return (
    <div className="rounded-md border border-slate-800 bg-slate-900/40 px-4 py-8 text-center text-sm text-slate-400">
      {message}
    </div>
  );
}

export function LoadingState({ message = "Loading…" }: { message?: string }) {
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
  children: ReactNode;
  tone?: keyof typeof badgeTones;
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
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section className="mb-6">
      <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        {title}
      </h2>
      <Card>
        <dl className="grid grid-cols-1 gap-x-6 gap-y-3 sm:grid-cols-2">
          {children}
        </dl>
      </Card>
    </section>
  );
}

/** One label/value pair inside a `<Section>`'s definition list. */
export function Field({
  label,
  value,
  full = false,
}: {
  label: string;
  value: ReactNode;
  full?: boolean;
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
  label: string;
  value: string;
  onChange: (value: string) => void;
  required?: boolean;
  placeholder?: string;
  full?: boolean;
  hint?: string;
  disabled?: boolean;
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
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  full?: boolean;
  hint?: string;
  rows?: number;
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
  label: string;
  value: number | "";
  onChange: (value: number | "") => void;
  required?: boolean;
  placeholder?: string;
  hint?: string;
  step?: number | string;
  min?: number;
  max?: number;
  full?: boolean;
  disabled?: boolean;
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
  label: string;
  value: Value;
  onChange: (value: Value) => void;
  options: readonly Option[];
  full?: boolean;
  placeholder?: string;
  required?: boolean;
  hint?: string;
  disabled?: boolean;
  displayLabel?: (value: Option) => string;
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
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  hint?: string;
  disabled?: boolean;
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
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section className="mb-6">
      <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        {title}
      </h2>
      <Card>
        <div className="grid grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-2">
          {children}
        </div>
      </Card>
    </section>
  );
}

/**
 * A centered overlay dialog: a fixed, full-viewport backdrop behind a bordered panel,
 * following the same dark `slate` theme as every other component here. First built for the
 * OPC tag-tree browser (`ui-opc-browser`) -- no earlier screen needed a true modal, since
 * `RunDetailPage`'s write/revert confirmations use the browser's native `window.confirm`
 * instead, which doesn't fit an interactive, multi-step tree browse. Closes on a backdrop
 * click, the header's close button, or Escape; a click inside the panel itself is stopped
 * from bubbling to the backdrop so interacting with the dialog's own content never closes it.
 */
export function Modal({
  title,
  onClose,
  children,
  widthClassName = "max-w-lg",
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  widthClassName?: string;
}) {
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-slate-950/70 p-4 pt-12"
      onClick={onClose}
    >
      <div
        className={`w-full ${widthClassName} rounded-lg border border-slate-700 bg-slate-900 shadow-xl`}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-slate-800 px-4 py-3">
          <h2 className="text-sm font-semibold text-slate-200">{title}</h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="text-slate-400 hover:text-slate-200"
          >
            ✕
          </button>
        </div>
        <div className="max-h-[70vh] overflow-y-auto p-4">{children}</div>
      </div>
    </div>
  );
}
