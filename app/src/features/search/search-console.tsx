import { type FormEvent, useState } from "react";
import type {
  CatalogGeneration,
  CatalogStatus,
  ChannelDetails,
  ChannelId,
  ClientResult,
  SparrowClient,
} from "../../client/contracts";
import {
  guidePresentation,
  type GuidePresentation,
} from "./guide-presentation";
import { ScheduleDesk } from "./schedule-desk";
import { SearchResultsPanel } from "./search-results";
import "./search-console.css";

const MAX_SEARCH_TERM_BYTES = 256;
const textEncoder = new TextEncoder();

interface SearchConsoleProps {
  readonly client: SparrowClient;
  readonly status: CatalogStatus | null;
  readonly catalogGeneration?: CatalogGeneration | null;
  readonly selectedChannel: ChannelId | null;
  readonly selectedDetails: ClientResult<ChannelDetails> | undefined;
  readonly selectedLoading: boolean;
  readonly onSelectChannel: (id: ChannelId) => void;
  readonly onRetrySelectedDetails: () => void;
}

/**
 * Searches the immutable catalog and presents one selected Channel schedule.
 * Search and schedule pagination retain successful pages when a later cursor fails.
 */
export function SearchConsole({
  client,
  status,
  catalogGeneration,
  selectedChannel,
  selectedDetails,
  selectedLoading,
  onSelectChannel,
  onRetrySelectedDetails,
}: SearchConsoleProps) {
  const [draft, setDraft] = useState("");
  const [submittedSearch, setSubmittedSearch] = useState<SubmittedSearch | null>(
    null,
  );
  const [validationMessage, setValidationMessage] = useState<string | null>(null);
  const [scheduleRevision, setScheduleRevision] = useState(0);
  const [scheduleFocusRevision, setScheduleFocusRevision] = useState(0);
  const guide = guidePresentation(status);
  const authoritativeGeneration =
    catalogGeneration === undefined ? status?.generation ?? null : catalogGeneration;

  const submitSearch = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const term = draft.trim();
    if (term.length === 0) {
      setValidationMessage("Enter a Channel or Programme name to scan the index.");
      return;
    }
    if (!fitsSearchContract(term)) {
      setValidationMessage(
        `Keep the search term within ${MAX_SEARCH_TERM_BYTES} UTF-8 bytes.`,
      );
      return;
    }
    setValidationMessage(null);
    setSubmittedSearch((current) => ({
      term,
      revision: (current?.revision ?? 0) + 1,
    }));
  };

  const restartSearch = () => {
    setSubmittedSearch((current) =>
      current === null
        ? null
        : { term: current.term, revision: current.revision + 1 },
    );
  };
  const selectSearchResult = (id: ChannelId) => {
    onSelectChannel(id);
    setScheduleFocusRevision((current) => current + 1);
  };

  return (
    <section className="search-console" aria-labelledby="search-console-heading">
      <header className="search-console-heading">
        <div className="search-index-mark" aria-hidden="true">
          02
        </div>
        <div>
          <p className="eyebrow">Cross-band index</p>
          <h2 id="search-console-heading">Find the next signal</h2>
        </div>
        <GuideBadge guide={guide} />
      </header>

      <form className="search-form" role="search" onSubmit={submitSearch}>
        <div className="search-input-wrap">
          <label htmlFor="catalog-search">Channel or Programme</label>
          <input
            id="catalog-search"
            name="query"
            type="search"
            autoComplete="off"
            maxLength={256}
            value={draft}
            aria-describedby={
              validationMessage === null ? "search-field-hint" : "search-field-error"
            }
            aria-invalid={validationMessage !== null}
            placeholder="News, cinema, a Programme title…"
            onChange={(event) => setDraft(event.currentTarget.value)}
          />
          <p
            id={
              validationMessage === null ? "search-field-hint" : "search-field-error"
            }
            className="search-field-note"
            role={validationMessage === null ? undefined : "alert"}
          >
            {validationMessage ??
              "Channel and Guide results stay on separate pagination tracks."}
          </p>
        </div>
        <button type="submit">Scan index</button>
      </form>

      <div className="search-workbench">
        {submittedSearch === null ? (
          <SearchIdle guide={guide} />
        ) : (
          <SearchResultsPanel
            key={`${submittedSearch.term}:${submittedSearch.revision}`}
            client={client}
            guide={guide}
            term={submittedSearch.term}
            revision={submittedSearch.revision}
            catalogGeneration={authoritativeGeneration}
            onRestart={restartSearch}
            onSelectChannel={selectSearchResult}
          />
        )}

        <ScheduleDesk
          key={selectedChannel ?? "no-channel"}
          client={client}
          guide={guide}
          selectedChannel={selectedChannel}
          selectedDetails={selectedDetails}
          selectedLoading={selectedLoading}
          revision={scheduleRevision}
          focusRevision={scheduleFocusRevision}
          catalogGeneration={authoritativeGeneration}
          onRestart={() => setScheduleRevision((current) => current + 1)}
          onRetrySelectedDetails={onRetrySelectedDetails}
        />
      </div>
    </section>
  );
}

interface SubmittedSearch {
  readonly term: string;
  readonly revision: number;
}

function fitsSearchContract(term: string): boolean {
  const canonical = term
    .normalize("NFKC")
    .toLowerCase()
    .trim()
    .replace(/\s+/gu, " ");
  return (
    textEncoder.encode(term).byteLength <= MAX_SEARCH_TERM_BYTES &&
    textEncoder.encode(canonical).byteLength <= MAX_SEARCH_TERM_BYTES
  );
}

function GuideBadge({ guide }: { readonly guide: GuidePresentation }) {
  return (
    <div className="guide-badge" data-tone={guide.tone}>
      <span aria-hidden="true" />
      <div>
        <small>EPG STATUS</small>
        <strong>{guide.label}</strong>
      </div>
    </div>
  );
}

function SearchIdle({ guide }: { readonly guide: GuidePresentation }) {
  return (
    <div className="search-idle">
      <div aria-hidden="true">A—Z</div>
      <section>
        <p className="eyebrow">Index standing by</p>
        <h3>One query. Two independent result lanes.</h3>
        <p>{guide.detail}</p>
      </section>
    </div>
  );
}
