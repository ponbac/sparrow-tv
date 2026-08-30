import { Check, HardDriveDownload, LockKeyhole, RadioTower } from "lucide-react";
import {
  type FormEvent,
  useEffect,
  useRef,
  useState,
} from "react";
import type {
  CatalogStatus,
  ClientError,
  InstalledSparrowClient,
} from "../../client/contracts";
import "./installed-source-settings.css";

/** Inputs for the private, on-device source configuration form. */
export interface InstalledSourceSettingsProps {
  readonly client: Pick<InstalledSparrowClient, "replaceSourceConfiguration">;
  readonly status: CatalogStatus | null;
  readonly onApplied: (status: CatalogStatus) => void;
}

type SaveState =
  | { readonly _tag: "idle" }
  | { readonly _tag: "saving" }
  | { readonly _tag: "saved" }
  | { readonly _tag: "failed"; readonly error: ClientError | null };

/**
 * Collects source locations only in uncontrolled DOM fields and sends them
 * directly to IPC. Values never enter React state, query caches, URLs, or logs.
 */
export function InstalledSourceSettings({
  client,
  status,
  onApplied,
}: InstalledSourceSettingsProps) {
  const m3uRef = useRef<HTMLInputElement>(null);
  const epgRef = useRef<HTMLInputElement>(null);
  const activeControllerRef = useRef<AbortController | null>(null);
  const mountedRef = useRef(false);
  const [saveState, setSaveState] = useState<SaveState>({ _tag: "idle" });

  useEffect(() => {
    const m3uField = m3uRef.current;
    const epgField = epgRef.current;
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      activeControllerRef.current?.abort();
      activeControllerRef.current = null;
      clearSourceFields(m3uField, epgField);
    };
  }, []);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (saveState._tag === "saving") {
      return;
    }
    const m3uLocation = m3uRef.current?.value.trim() ?? "";
    const epgDraft = epgRef.current?.value.trim() ?? "";
    if (m3uLocation.length === 0) {
      setSaveState({
        _tag: "failed",
        error: {
          _tag: "invalid-input",
          field: "m3u",
          reason: "required",
        },
      });
      m3uRef.current?.focus();
      return;
    }

    const controller = new AbortController();
    activeControllerRef.current = controller;
    setSaveState({ _tag: "saving" });
    client
      .replaceSourceConfiguration({
        m3uLocation,
        epgLocation: epgDraft.length === 0 ? null : epgDraft,
        signal: controller.signal,
      })
      .then(
        (result) => {
          if (
            !mountedRef.current ||
            activeControllerRef.current !== controller
          ) {
            return;
          }
          activeControllerRef.current = null;
          if (result.ok) {
            clearSourceFields(m3uRef.current, epgRef.current);
            setSaveState({ _tag: "saved" });
            onApplied(result.value);
            return;
          }
          if (result.error._tag === "cancelled") {
            setSaveState({ _tag: "idle" });
            return;
          }
          setSaveState({ _tag: "failed", error: result.error });
        },
        () => {
          if (
            mountedRef.current &&
            activeControllerRef.current === controller
          ) {
            activeControllerRef.current = null;
            setSaveState({ _tag: "failed", error: null });
          }
        },
      );
  };

  const resetFeedback = () => {
    if (saveState._tag === "saved" || saveState._tag === "failed") {
      setSaveState({ _tag: "idle" });
    }
  };
  const configured = status?.configuration.configured === true;

  return (
    <section
      className="installed-settings"
      aria-labelledby="installed-settings-heading"
    >
      <header className="installed-settings__heading">
        <div className="installed-settings__index" aria-hidden="true">
          CFG
        </div>
        <div>
          <p className="eyebrow">On-device source cabinet</p>
          <h2 id="installed-settings-heading">
            {configured ? "Replace local sources" : "Tune this receiver"}
          </h2>
        </div>
        <span className="installed-settings__privacy">
          <LockKeyhole aria-hidden="true" />
          Device private
        </span>
      </header>

      <div className="installed-settings__body">
        <div className="installed-settings__brief">
          <RadioTower aria-hidden="true" />
          <p>
            Add one M3U source and, if available, one XMLTV guide. Sparrow stores
            them in the installed app and returns only safe catalog status to this
            screen.
          </p>
        </div>

        <form
          className="installed-settings__form"
          autoComplete="off"
          aria-busy={saveState._tag === "saving"}
          onSubmit={submit}
        >
          <label htmlFor="installed-m3u-source">
            <span>Required / Channel source</span>
            <input
              ref={m3uRef}
              id="installed-m3u-source"
              type="url"
              inputMode="url"
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              maxLength={16_384}
              required
              placeholder="https://…/channels.m3u"
              onInput={resetFeedback}
            />
          </label>
          <label htmlFor="installed-epg-source">
            <span>Optional / Guide source</span>
            <input
              ref={epgRef}
              id="installed-epg-source"
              type="url"
              inputMode="url"
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              maxLength={16_384}
              placeholder="https://…/guide.xml"
              onInput={resetFeedback}
            />
          </label>
          <button type="submit" disabled={saveState._tag === "saving"}>
            {saveState._tag === "saving" ? (
              <HardDriveDownload aria-hidden="true" />
            ) : saveState._tag === "saved" ? (
              <Check aria-hidden="true" />
            ) : (
              <RadioTower aria-hidden="true" />
            )}
            {saveState._tag === "saving"
              ? "Validating & saving"
              : saveState._tag === "saved"
                ? "Sources saved"
                : configured
                  ? "Replace sources"
                  : "Build local catalog"}
          </button>
        </form>

        <SaveFeedback state={saveState} />
      </div>
    </section>
  );
}

function SaveFeedback({ state }: { readonly state: SaveState }) {
  if (state._tag === "idle" || state._tag === "saving") {
    return (
      <p className="installed-settings__note">
        Source locations are never placed in browser history or diagnostics.
      </p>
    );
  }
  if (state._tag === "saved") {
    return (
      <p className="installed-settings__feedback" data-tone="saved" role="status">
        Configuration saved. Safe catalog status will update as the local build
        completes.
      </p>
    );
  }
  return (
    <p className="installed-settings__feedback" data-tone="failed" role="alert">
      {configurationErrorCopy(state.error)}
    </p>
  );
}

function configurationErrorCopy(error: ClientError | null): string {
  if (error?._tag === "invalid-input") {
    if (error.field === "m3u" && error.reason === "required") {
      return "Enter a Channel source before building the local catalog.";
    }
    return "One source location is not supported. Check it and try again.";
  }
  if (error?._tag === "catalog-unavailable") {
    return "The sources were saved, but no valid local catalog is available yet.";
  }
  return "The installed app could not save this configuration. Try again.";
}

function clearSourceFields(
  m3u: HTMLInputElement | null,
  epg: HTMLInputElement | null,
): void {
  if (m3u !== null) {
    m3u.value = "";
  }
  if (epg !== null) {
    epg.value = "";
  }
}
