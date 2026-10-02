/**
 * Small shared Tailwind building blocks so the status-coloring convention established in
 * the `frontend-shell` health indicator (slate = pending/neutral, red = error, emerald =
 * success) stays consistent across every screen rather than being re-invented per file.
 */
import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";

const modalStack: HTMLDialogElement[] = [];
const modalStackListeners = new Set<() => void>();

function syncModalStack() {
  const topmostModal = modalStack.at(-1);
  for (const dialog of modalStack) {
    const isTopmost = dialog === topmostModal;
    dialog.inert = !isTopmost;
    if (isTopmost) {
      dialog.removeAttribute("aria-hidden");
      dialog.setAttribute("aria-modal", "true");
    } else {
      dialog.setAttribute("aria-hidden", "true");
      dialog.removeAttribute("aria-modal");
    }
  }
  for (const listener of modalStackListeners) listener();
}

const FOCUSABLE_SELECTOR =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function focusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
  ).filter(
    (element) =>
      element.tabIndex >= 0 &&
      !element.closest('[aria-hidden="true"], [inert], [hidden]') &&
      element.getClientRects().length > 0,
  );
}

export function PageHeading({
  title,
  description,
  titleActions,
  actions,
  documentationId,
}: {
  readonly title: string;
  readonly description?: string;
  readonly titleActions?: ReactNode;
  readonly actions?: ReactNode;
  readonly documentationId?: string;
}) {
  return (
    <div
      className="mb-6 flex flex-col items-start justify-between gap-4 min-[1025px]:flex-row"
      data-doc-section={documentationId}
    >
      <div className="flex min-w-0 flex-wrap items-start gap-3">
        <div className="min-w-0">
          <h1 className="break-words text-2xl font-semibold tracking-tight">
            {title}
          </h1>
          {description && (
            <p className="mt-1 text-sm text-slate-400">{description}</p>
          )}
        </div>
        {titleActions}
      </div>
      {actions && (
        <div className="flex w-full flex-wrap gap-2 min-[1025px]:w-auto min-[1025px]:justify-end">
          {actions}
        </div>
      )}
    </div>
  );
}

const buttonVariants = {
  primary:
    "border-emerald-800 bg-emerald-900/40 text-emerald-300 hover:bg-emerald-900/70",
  danger: "border-red-800 bg-red-950 text-red-300 hover:bg-red-900/60",
  neutral: "border-slate-700 bg-slate-900 text-slate-200 hover:bg-slate-800",
} as const;

type SpinnerSize = "sm" | "md" | "lg";

const spinnerSizes: Record<SpinnerSize, string> = {
  sm: "h-3.5 w-3.5 border-2",
  md: "h-5 w-5 border-2",
  lg: "h-8 w-8 border-[3px]",
};

/** A decorative CSS-only spinner. Pair it with visible status text for accessibility. */
export function Spinner({ size = "md" }: { readonly size?: SpinnerSize }) {
  return (
    <span
      aria-hidden="true"
      className={`inline-block shrink-0 rounded-full border-slate-600 border-t-emerald-400 motion-safe:animate-spin motion-reduce:animate-none ${spinnerSizes[size]}`}
    />
  );
}

/** An accessible loading message for inline, panel, and dialog operations. */
export function LoadingStatus({
  message,
  size = "md",
  className = "",
}: {
  readonly message: string;
  readonly size?: SpinnerSize;
  readonly className?: string;
}) {
  return (
    <div
      role="status"
      aria-live="polite"
      aria-busy="true"
      className={`flex items-center gap-2 ${className}`}
    >
      <Spinner size={size} />
      <span>{message}</span>
    </div>
  );
}

/**
 * Covers an active region while keeping its children mounted for layout measurement and
 * preserving their state. The opaque layer blocks pointer interaction and removes the covered
 * subtree from the accessibility tree until the operation settles.
 */
export function LoadingOverlay({
  active,
  message,
  children,
  className = "",
}: {
  readonly active: boolean;
  readonly message: string;
  readonly children: ReactNode;
  readonly className?: string;
}) {
  return (
    <div className={`relative ${className}`} aria-busy={active || undefined}>
      <div
        aria-hidden={active || undefined}
        inert={active}
        className={active ? "pointer-events-none select-none" : undefined}
      >
        {children}
      </div>
      {active && (
        <LoadingStatus
          message={message}
          size="lg"
          className="absolute inset-0 z-20 justify-center bg-slate-900/95 px-4 text-sm text-slate-300"
        />
      )}
    </div>
  );
}

export function Button({
  children,
  onClick,
  type = "button",
  variant = "neutral",
  disabled = false,
  loading = false,
  title,
  buttonRef,
}: {
  readonly children: ReactNode;
  readonly onClick?: () => void;
  readonly type?: "button" | "submit";
  readonly variant?: keyof typeof buttonVariants;
  readonly disabled?: boolean;
  readonly loading?: boolean;
  readonly title?: string;
  readonly buttonRef?: RefObject<HTMLButtonElement | null>;
}) {
  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      title={title}
      ref={buttonRef}
      className={`rounded-md border px-3 py-1.5 text-sm font-medium disabled:cursor-not-allowed disabled:bg-slate-800 disabled:text-slate-100 ${buttonVariants[variant]}`}
    >
      <span className="inline-flex items-center gap-2">
        {loading && <Spinner size="sm" />}
        {children}
      </span>
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
  documentationId,
}: {
  readonly title: string;
  readonly children: ReactNode;
  readonly defaultOpen?: boolean;
  readonly trailing?: ReactNode;
  readonly className?: string;
  readonly documentationId?: string;
}) {
  const [isOpen, setIsOpen] = useState(defaultOpen);

  return (
    <details
      className={`group ${className}`}
      open={isOpen}
      onToggle={(event) => setIsOpen(event.currentTarget.open)}
      data-doc-section={documentationId}
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

const inlineStatusTones = {
  success: "border-emerald-800 bg-emerald-950/50 text-emerald-300",
  error: "border-red-800 bg-red-950/50 text-red-300",
  warning: "border-amber-800 bg-amber-950/50 text-amber-300",
  info: "border-slate-700 bg-slate-900/60 text-slate-300",
} as const;

export function InlineStatus({
  message,
  tone = "success",
  className = "",
}: {
  readonly message: string;
  readonly tone?: keyof typeof inlineStatusTones;
  readonly className?: string;
}) {
  const isError = tone === "error";
  return (
    <div
      role={isError ? "alert" : "status"}
      aria-live={isError ? "assertive" : "polite"}
      aria-atomic="true"
      className={`rounded-md border px-3 py-2 text-sm ${inlineStatusTones[tone]} ${className}`}
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
    <div className="rounded-md border border-slate-700 bg-slate-900 px-4 py-8 text-sm text-slate-400">
      <LoadingStatus message={message} className="justify-center" />
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
  documentationId,
}: {
  readonly title: string;
  readonly children: ReactNode;
  readonly collapsible?: boolean;
  readonly defaultOpen?: boolean;
  readonly documentationId?: string;
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
      <CollapsibleSection
        title={title}
        defaultOpen={defaultOpen}
        documentationId={documentationId}
      >
        {content}
      </CollapsibleSection>
    );
  }

  return (
    <section className="mb-6" data-doc-section={documentationId}>
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
  "mt-1 w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 placeholder:text-slate-500 focus:border-slate-500 focus:outline-none";

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
  error,
}: {
  readonly label: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly required?: boolean;
  readonly placeholder?: string;
  readonly full?: boolean;
  readonly hint?: string;
  readonly disabled?: boolean;
  readonly error?: string;
}) {
  const id = useId();
  const visibleError = disabled ? undefined : error;
  const hintId = hint ? `${id}-hint` : undefined;
  const errorId = visibleError ? `${id}-error` : undefined;
  return (
    <div className={`block ${full ? "sm:col-span-2" : ""}`}>
      <label
        htmlFor={id}
        className="text-xs uppercase tracking-wide text-slate-500"
      >
        {label}
        {required && (
          <span aria-hidden="true" className="ml-1 text-red-400">
            *
          </span>
        )}
      </label>
      <input
        id={id}
        type="text"
        value={value}
        placeholder={placeholder}
        required={required && !disabled}
        disabled={disabled}
        aria-invalid={visibleError ? true : undefined}
        aria-describedby={
          [hintId, errorId].filter(Boolean).join(" ") || undefined
        }
        onChange={(e) => onChange(e.target.value)}
        className={`${fieldControlClass} disabled:cursor-not-allowed disabled:bg-slate-800/50`}
      />
      <FieldFeedback
        hint={hint}
        hintId={hintId}
        error={visibleError}
        errorId={errorId}
      />
    </div>
  );
}

function FieldFeedback({
  hint,
  hintId,
  error,
  errorId,
  className = "",
}: {
  readonly hint?: string;
  readonly hintId?: string;
  readonly error?: string;
  readonly errorId?: string;
  readonly className?: string;
}) {
  if (!hint && !error) return null;
  return (
    <div className={`mt-1 space-y-1 ${className}`}>
      {hint && (
        <span id={hintId} className="block text-xs text-slate-500">
          {hint}
        </span>
      )}
      {error && (
        <span id={errorId} className="block text-xs text-red-300">
          {error}
        </span>
      )}
    </div>
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
  error,
}: {
  readonly label: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly placeholder?: string;
  readonly full?: boolean;
  readonly hint?: string;
  readonly rows?: number;
  readonly error?: string;
}) {
  const id = useId();
  const hintId = hint ? `${id}-hint` : undefined;
  const errorId = error ? `${id}-error` : undefined;
  return (
    <div className={`block ${full ? "sm:col-span-2" : ""}`}>
      <label
        htmlFor={id}
        className="text-xs uppercase tracking-wide text-slate-500"
      >
        {label}
      </label>
      <textarea
        id={id}
        value={value}
        placeholder={placeholder}
        rows={rows}
        aria-invalid={error ? true : undefined}
        aria-describedby={
          [hintId, errorId].filter(Boolean).join(" ") || undefined
        }
        onChange={(e) => onChange(e.target.value)}
        className={`${fieldControlClass} resize-y`}
      />
      <FieldFeedback
        hint={hint}
        hintId={hintId}
        error={error}
        errorId={errorId}
      />
    </div>
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
  error,
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
  readonly error?: string;
}) {
  const id = useId();
  const visibleError = disabled ? undefined : error;
  const hintId = hint ? `${id}-hint` : undefined;
  const errorId = visibleError ? `${id}-error` : undefined;
  return (
    <div className={`block ${full ? "sm:col-span-2" : ""}`}>
      <label
        htmlFor={id}
        className="text-xs uppercase tracking-wide text-slate-500"
      >
        {label}
        {required && (
          <span aria-hidden="true" className="ml-1 text-red-400">
            *
          </span>
        )}
      </label>
      <input
        id={id}
        type="number"
        value={value}
        placeholder={placeholder}
        step={step}
        min={min}
        max={max}
        required={required && !disabled}
        disabled={disabled}
        aria-invalid={visibleError ? true : undefined}
        aria-describedby={
          [hintId, errorId].filter(Boolean).join(" ") || undefined
        }
        onChange={(e) =>
          onChange(e.target.value === "" ? "" : e.target.valueAsNumber)
        }
        className={`${fieldControlClass} disabled:cursor-not-allowed disabled:bg-slate-800/50`}
      />
      <FieldFeedback
        hint={hint}
        hintId={hintId}
        error={visibleError}
        errorId={errorId}
      />
    </div>
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
  error,
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
  readonly error?: string;
}) {
  const id = useId();
  const visibleError = disabled ? undefined : error;
  const hintId = hint ? `${id}-hint` : undefined;
  const errorId = visibleError ? `${id}-error` : undefined;
  return (
    <div className={`block ${full ? "sm:col-span-2" : ""}`}>
      <label
        htmlFor={id}
        className="text-xs uppercase tracking-wide text-slate-500"
      >
        {label}
        {required && (
          <span aria-hidden="true" className="ml-1 text-red-400">
            *
          </span>
        )}
      </label>
      <select
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value as Value)}
        required={required && !disabled}
        disabled={disabled}
        aria-invalid={visibleError ? true : undefined}
        aria-describedby={
          [hintId, errorId].filter(Boolean).join(" ") || undefined
        }
        className={`${fieldControlClass} disabled:cursor-not-allowed disabled:bg-slate-800/50`}
      >
        {placeholder !== undefined && <option value="">{placeholder}</option>}
        {options.map((option) => (
          <option key={option} value={option}>
            {displayLabel ? displayLabel(option) : option}
          </option>
        ))}
      </select>
      <FieldFeedback
        hint={hint}
        hintId={hintId}
        error={visibleError}
        errorId={errorId}
      />
    </div>
  );
}

/** A labeled checkbox, styled to line up with `<TextField>`/`<SelectField>` in a grid. */
export function CheckboxField({
  label,
  checked,
  onChange,
  hint,
  disabled = false,
  error,
}: {
  readonly label: string;
  readonly checked: boolean;
  readonly onChange: (checked: boolean) => void;
  readonly hint?: string;
  readonly disabled?: boolean;
  readonly error?: string;
}) {
  const id = useId();
  const visibleError = disabled ? undefined : error;
  const hintId = hint ? `${id}-hint` : undefined;
  const errorId = visibleError ? `${id}-error` : undefined;
  return (
    <div className="pt-5">
      <div className="flex items-start gap-2">
        <input
          id={id}
          type="checkbox"
          checked={checked}
          aria-invalid={visibleError ? true : undefined}
          aria-describedby={
            [hintId, errorId].filter(Boolean).join(" ") || undefined
          }
          onChange={(e) => onChange(e.target.checked)}
          disabled={disabled}
          className="mt-0.5 h-4 w-4 rounded border-slate-700 bg-slate-950 disabled:cursor-not-allowed disabled:bg-slate-800"
        />
        <label
          htmlFor={id}
          className={`text-sm ${disabled ? "text-slate-400" : "text-slate-200"}`}
        >
          {label}
        </label>
      </div>
      <FieldFeedback
        hint={hint}
        hintId={hintId}
        error={visibleError}
        errorId={errorId}
        className="ml-6"
      />
    </div>
  );
}

export function FormSection({
  title,
  children,
  collapsible = false,
  defaultOpen = false,
  documentationId,
}: {
  readonly title: string;
  readonly children: ReactNode;
  readonly collapsible?: boolean;
  readonly defaultOpen?: boolean;
  readonly documentationId?: string;
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
      <CollapsibleSection
        title={title}
        defaultOpen={defaultOpen}
        documentationId={documentationId}
      >
        {content}
      </CollapsibleSection>
    );
  }

  return (
    <section className="mb-6" data-doc-section={documentationId}>
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
  initialFocusRef,
  restoreFocusRef,
  documentationId,
}: {
  readonly title: string;
  readonly onClose: () => void;
  readonly children: ReactNode;
  readonly widthClassName?: string;
  readonly dismissible?: boolean;
  readonly initialFocusRef?: RefObject<HTMLElement | null>;
  readonly restoreFocusRef?: RefObject<HTMLElement | null>;
  readonly documentationId?: string;
}) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const onCloseRef = useRef(onClose);
  const dismissibleRef = useRef(dismissible);
  const [isTopmostModal, setIsTopmostModal] = useState(true);
  useEffect(() => {
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = previousOverflow;
    };
  }, []);

  useLayoutEffect(() => {
    onCloseRef.current = onClose;
    dismissibleRef.current = dismissible;
  }, [onClose, dismissible]);

  useLayoutEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    const previouslyFocused =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const focusReturnTarget = restoreFocusRef?.current ?? previouslyFocused;
    const updateTopmostState = () => {
      setIsTopmostModal(modalStack.at(-1) === dialog);
    };
    modalStack.push(dialog);
    modalStackListeners.add(updateTopmostState);
    syncModalStack();

    const focusable = focusableElements(dialog);
    const preferredFocus = initialFocusRef?.current;
    const target =
      preferredFocus &&
      dialog.contains(preferredFocus) &&
      focusable.includes(preferredFocus)
        ? preferredFocus
        : (focusable[0] ?? dialog);
    target.focus({ preventScroll: true });

    function onFocusIn(event: FocusEvent) {
      if (
        modalStack.at(-1) !== dialog ||
        !(event.target instanceof Node) ||
        dialog.contains(event.target)
      ) {
        return;
      }

      const firstFocusable = focusableElements(dialog)[0];
      (firstFocusable ?? dialog).focus({ preventScroll: true });
    }

    function onKeyDown(event: KeyboardEvent) {
      if (modalStack.at(-1) !== dialog) return;

      if (event.key === "Escape") {
        event.preventDefault();
        if (dismissibleRef.current) onCloseRef.current();
        return;
      }
      if (event.key !== "Tab") return;

      const currentFocusableElements = focusableElements(dialog);
      const first = currentFocusableElements[0];
      if (!first) {
        event.preventDefault();
        dialog.focus({ preventScroll: true });
        return;
      }
      const last = currentFocusableElements.at(-1);
      const active = document.activeElement;
      const activeIndex =
        active instanceof HTMLElement
          ? currentFocusableElements.indexOf(active)
          : -1;
      if (event.shiftKey && activeIndex <= 0) {
        event.preventDefault();
        last?.focus();
      } else if (
        !event.shiftKey &&
        (activeIndex === -1 ||
          activeIndex === currentFocusableElements.length - 1)
      ) {
        event.preventDefault();
        first.focus();
      }
    }
    document.addEventListener("focusin", onFocusIn);
    dialog.addEventListener("keydown", onKeyDown);

    return () => {
      document.removeEventListener("focusin", onFocusIn);
      dialog.removeEventListener("keydown", onKeyDown);
      modalStackListeners.delete(updateTopmostState);
      const stackIndex = modalStack.lastIndexOf(dialog);
      if (stackIndex >= 0) modalStack.splice(stackIndex, 1);
      syncModalStack();

      window.requestAnimationFrame(() => {
        if (
          dialog.isConnected ||
          !focusReturnTarget?.isConnected ||
          focusReturnTarget.closest('[aria-hidden="true"], [inert]') ||
          focusReturnTarget.hasAttribute("disabled")
        ) {
          return;
        }
        focusReturnTarget.focus({ preventScroll: true });
      });
    };
  }, [initialFocusRef, restoreFocusRef]);

  return createPortal(
    <div
      className="modal-backdrop fixed inset-0 z-50 flex items-center justify-center overflow-y-auto p-4"
      data-doc-section={documentationId}
    >
      <button
        type="button"
        aria-label="Dismiss modal backdrop"
        onClick={onClose}
        disabled={!dismissible}
        tabIndex={-1}
        className="absolute inset-0 cursor-default disabled:cursor-not-allowed"
      />
      <dialog
        ref={dialogRef}
        open
        tabIndex={-1}
        aria-modal={isTopmostModal ? "true" : undefined}
        aria-hidden={!isTopmostModal || undefined}
        inert={!isTopmostModal}
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
            className="text-slate-400 hover:text-slate-200 disabled:cursor-not-allowed"
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

export function ConfirmModal({
  title,
  children,
  onCancel,
  onConfirm,
  pending,
  confirmLabel,
  pendingLabel,
  errorMessage,
  confirmVariant = "danger",
  documentationId,
}: {
  readonly title: string;
  readonly children: ReactNode;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
  readonly pending: boolean;
  readonly confirmLabel: string;
  readonly pendingLabel: string;
  readonly errorMessage?: string | null;
  readonly confirmVariant?: keyof typeof buttonVariants;
  readonly documentationId?: string;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  return (
    <Modal
      title={title}
      onClose={onCancel}
      dismissible={!pending}
      initialFocusRef={cancelRef}
      documentationId={documentationId}
    >
      <div className="space-y-4">
        <div className="text-sm text-slate-300">{children}</div>
        {errorMessage && <ErrorBanner message={errorMessage} />}
        {pending && (
          <div
            role="status"
            aria-live="polite"
            className="rounded-md border border-slate-700 bg-slate-950/60 px-4 py-3 text-sm text-slate-300"
          >
            {pendingLabel} Do not close this dialog.
          </div>
        )}
        <div className="flex justify-end gap-2">
          <Button onClick={onCancel} disabled={pending} buttonRef={cancelRef}>
            Cancel
          </Button>
          <Button
            variant={confirmVariant}
            onClick={onConfirm}
            disabled={pending}
          >
            {pending ? pendingLabel : confirmLabel}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
